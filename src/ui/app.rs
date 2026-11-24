use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, HDC, PAINTSTRUCT};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_SPACE, VK_UP,
};
use windows::Win32::UI::Shell::NIN_SELECT;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetWindowLongPtrW,
    GetWindowRect, HWND_TOPMOST, IDC_ARROW, IDC_HAND, KillTimer, LoadCursorW, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, PostQuitMessage, QS_ALLINPUT, SW_HIDE,
    SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetCursor, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WA_INACTIVE, WM_ACTIVATE,
    WM_CONTEXTMENU, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_QUIT, WM_SETCURSOR, WM_SETTINGCHANGE,
    WM_TIMER,
};

use crate::core::error::{AppError, AppResult};
use crate::core::model::TrayIconMode;
use crate::platform::SystemLayoutManager;
use crate::platform::active_input::{
    capture_external_foreground, foreground_hkl, notify_window_layout_change,
};
use crate::platform::apply_worker::{ApplyOutcome, ApplyWorker, WM_APPLY_COMPLETE};
use crate::platform::shell::{is_autostart_enabled, open_language_settings, set_autostart_enabled};
use crate::platform::single_instance::SingleInstanceGuard;
use crate::platform::theme::{SystemTheme, apply_flyout_dwm};
use crate::platform::watcher::RegistryWatcher;
use crate::ui::animation::FlyoutAnimator;
use crate::ui::graphics::GraphicsFactories;
use crate::ui::interaction::FlyoutInteractionState;
use crate::ui::layout::{FlyoutGeometry, HitTarget, compute_flyout_position};
use crate::ui::palette::FluentPalette;
use crate::ui::renderer::{FlyoutRenderView, FlyoutRenderer, RowRenderItem};
use crate::ui::tray::{TrayController, TrayMenuAction, WM_TRAY_CALLBACK};
use crate::ui::window::{
    WindowGuard, default_anchor_point, dip_size_to_px, lparam_coords, monitor_work_area_and_dpi,
    px_point_to_dip, register_taskbar_created_message,
};

const NIN_KEYSELECT: u32 = NIN_SELECT + 1;

const TIMER_ACTIVE_POLL: usize = 2;
const TIMER_REG_DEBOUNCE: usize = 3;
const TIMER_NOTICE_EXPIRE: usize = 4;

const ACTIVE_POLL_MS: u32 = 250;
const REG_DEBOUNCE_MS: u32 = 380;
const NOTICE_DURATION_MS: u32 = 2600;
const SELF_MUTATION_SUPPRESS: Duration = Duration::from_millis(950);
const DISMISS_REOPEN_GUARD: Duration = Duration::from_millis(220);
const OPEN_ACTIVATE_GUARD: Duration = Duration::from_millis(120);

/// Coordinates the flyout window, domain manager, async apply worker, tray icon,
/// animation engine, and user interaction state machine.
struct FlyoutApp {
    hwnd: HWND,
    manager: SystemLayoutManager,
    worker: ApplyWorker,
    tray: TrayController,
    renderer: FlyoutRenderer,
    animator: FlyoutAnimator,
    interaction: FlyoutInteractionState,
    theme: SystemTheme,
    palette: FluentPalette,
    dpi: u32,
    visible: bool,
    tracking_mouse: bool,
    prev_foreground: Option<HWND>,
    last_polled_hkl: Option<isize>,
    active_index: Option<usize>,
    last_dismissed_at: Option<Instant>,
    suppress_reg_until: Option<Instant>,
    ignore_deactivate_until: Option<Instant>,
    status_notice: Option<String>,
    wake_message: u32,
    taskbar_created_msg: u32,
}

/// Runs the resident Windows 11 system tray + native Acrylic flyout application.
pub fn run_tray_app(manager: SystemLayoutManager, show_immediately: bool) -> AppResult<()> {
    let Some(single_instance) = SingleInstanceGuard::acquire_or_wake()? else {
        return Ok(());
    };

    let wake_message = single_instance.wake_message();
    let taskbar_created_msg = register_taskbar_created_message();
    let theme = SystemTheme::read();
    let palette = FluentPalette::from_theme(theme);

    let window = WindowGuard::create_flyout(theme, Some(wnd_proc))?;
    let hwnd = window.raw();
    // SAFETY: Querying DPI of our newly created window.
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);

    let fg_hkl = foreground_hkl();
    let active_index = resolve_active_index(&manager, fg_hkl);
    let initial_code = manager.keycap_code_for(active_index);
    let initial_mode = manager.tray_icon_mode();

    let factories = GraphicsFactories::new()?;
    let tray = TrayController::new(
        hwnd,
        factories.clone(),
        initial_mode,
        &initial_code,
        theme.taskbar_dark,
        dpi,
    )?;
    let worker = ApplyWorker::spawn(hwnd)?;

    let geom = FlyoutGeometry::compute(
        manager.state().profiles.len(),
        manager.has_disabled_profiles(),
        0.0,
    );
    let (w_px, h_px) = dip_size_to_px(geom.width, geom.height, dpi);
    let renderer = FlyoutRenderer::new(hwnd, &factories, w_px, h_px, dpi)?;
    let animator = FlyoutAnimator::new(&manager.state().profiles, Instant::now());

    let app = Box::new(RefCell::new(FlyoutApp {
        hwnd,
        manager,
        worker,
        tray,
        renderer,
        animator,
        interaction: FlyoutInteractionState::default(),
        theme,
        palette,
        dpi,
        visible: false,
        tracking_mouse: false,
        prev_foreground: None,
        last_polled_hkl: fg_hkl,
        active_index,
        last_dismissed_at: None,
        suppress_reg_until: None,
        ignore_deactivate_until: None,
        status_notice: None,
        wake_message,
        taskbar_created_msg,
    }));

    let app_ptr = Box::into_raw(app);
    // SAFETY: Storing owned pointer in `GWLP_USERDATA`; reclaimed cleanly after message loop exits.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app_ptr as isize);
        let _ = SetTimer(hwnd, TIMER_ACTIVE_POLL, ACTIVE_POLL_MS, None);
    }

    if show_immediately {
        // SAFETY: `app_ptr` is uniquely owned by this thread and valid.
        unsafe {
            (*app_ptr).borrow_mut().open_flyout(None);
        }
    }

    let reg_watcher = RegistryWatcher::start().ok();
    run_message_loop(hwnd, app_ptr, reg_watcher.as_ref());

    // SAFETY: Detach `GWLP_USERDATA` before destroying the window and dropping `FlyoutApp`.
    unsafe {
        let _ = KillTimer(hwnd, TIMER_ACTIVE_POLL);
        let _ = KillTimer(hwnd, TIMER_REG_DEBOUNCE);
        let _ = KillTimer(hwnd, TIMER_NOTICE_EXPIRE);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(window);
        drop(Box::from_raw(app_ptr));
    }

    Ok(())
}

/// Hybrid VSync + zero-CPU-idle message loop:
/// - While animations are active, locks cleanly to the monitor refresh rate via `DwmFlush()` (60–240 Hz).
/// - When idle, sleeps indefinitely inside `MsgWaitForMultipleObjects` with 0% CPU usage.
fn run_message_loop(
    hwnd: HWND,
    app_ptr: *mut RefCell<FlyoutApp>,
    reg_watcher: Option<&RegistryWatcher>,
) {
    let mut msg = MSG::default();
    let handles = reg_watcher.map(RegistryWatcher::handles);

    loop {
        // SAFETY: Draining all pending Win32 messages before either stepping a VSync frame or sleeping.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let animating = (*app_ptr)
                .try_borrow()
                .is_ok_and(|a| a.visible && a.animator.is_active());

            if animating {
                if let Ok(mut app) = (*app_ptr).try_borrow_mut() {
                    app.step_vsync_frame();
                }
                if DwmFlush().is_err() {
                    std::thread::sleep(Duration::from_millis(8));
                }
                continue;
            }

            let wait_res = match &handles {
                Some(h) => MsgWaitForMultipleObjects(Some(h), false, u32::MAX, QS_ALLINPUT),
                None => MsgWaitForMultipleObjects(None, false, u32::MAX, QS_ALLINPUT),
            };

            if let Some(watcher) = reg_watcher
                && (wait_res.0 == 0 || wait_res.0 == 1)
            {
                let _ = watcher.reset_and_rearm(wait_res.0 as usize);
                if let Ok(app) = (*app_ptr).try_borrow() {
                    let suppressed = app
                        .suppress_reg_until
                        .is_some_and(|until| Instant::now() < until);
                    if !suppressed {
                        let _ = SetTimer(hwnd, TIMER_REG_DEBOUNCE, REG_DEBOUNCE_MS, None);
                    }
                }
            }
        }
    }
}

impl FlyoutApp {
    fn open_flyout(&mut self, anchor: Option<POINT>) {
        self.prev_foreground = capture_external_foreground(self.hwnd);
        let _ = self.manager.sync_with_system();
        self.animator.sync_profiles(&self.manager.state().profiles);
        self.active_index = resolve_active_index(&self.manager, foreground_hkl());
        self.refresh_theme_if_needed();

        let pt = anchor.unwrap_or_else(default_anchor_point);
        let (work_area, target_dpi) = monitor_work_area_and_dpi(pt);
        self.dpi = target_dpi;

        let geom = self.current_geometry();
        let (w_px, h_px) = dip_size_to_px(geom.width, geom.height, self.dpi);
        let (x, y) =
            compute_flyout_position(pt.x, pt.y, work_area, w_px as i32, h_px as i32, self.dpi);

        self.interaction.reset_transient();

        let now = Instant::now();
        self.animator.on_open(now);
        self.ignore_deactivate_until = Some(now + OPEN_ACTIVATE_GUARD);
        self.visible = true;

        // SAFETY: Positioning and displaying our owned topmost flyout window.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                w_px as i32,
                h_px as i32,
                SWP_NOACTIVATE,
            );
        }

        let _ = self.renderer.resize(w_px, h_px, self.dpi);
        self.render_now();

        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    fn hide_flyout(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.animator.on_hide();
        self.interaction.reset_transient();
        self.tracking_mouse = false;
        self.last_dismissed_at = Some(Instant::now());
        // SAFETY: Hiding our owned flyout window cleanly.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn toggle_flyout(&mut self, anchor: Option<POINT>) {
        if self.visible {
            self.hide_flyout();
            return;
        }
        if self
            .last_dismissed_at
            .is_some_and(|t| t.elapsed() < DISMISS_REOPEN_GUARD)
        {
            return;
        }
        self.open_flyout(anchor);
    }

    fn current_geometry(&self) -> FlyoutGeometry {
        FlyoutGeometry::compute(
            self.manager.state().profiles.len(),
            self.manager.has_disabled_profiles(),
            self.interaction.scroll_offset,
        )
    }

    fn hit_test_lparam(&self, lparam: LPARAM) -> HitTarget {
        let (x_px, y_px) = lparam_coords(lparam);
        let (x_dip, y_dip) = px_point_to_dip(x_px, y_px, self.dpi);
        self.current_geometry().hit_test(x_dip, y_dip)
    }

    fn resize_if_visible(&mut self) {
        if !self.visible {
            return;
        }
        let geom = self.current_geometry();
        let (w_px, h_px) = dip_size_to_px(geom.width, geom.height, self.dpi);

        let mut cur_rect = RECT::default();
        // SAFETY: Querying our own window's current screen rectangle.
        let got_rect = unsafe { GetWindowRect(self.hwnd, &mut cur_rect).is_ok() };
        unsafe {
            if got_rect {
                let new_top = cur_rect.bottom - h_px as i32;
                let _ = SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    cur_rect.left,
                    new_top,
                    w_px as i32,
                    h_px as i32,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            } else {
                let _ = SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    w_px as i32,
                    h_px as i32,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
                );
            }
        }
        let _ = self.renderer.resize(w_px, h_px, self.dpi);
    }

    fn handle_tray_event(&mut self, lparam: LPARAM) {
        // Under NOTIFYICON_VERSION_4, Windows emits BOTH `WM_LBUTTONUP` and `NIN_SELECT` on one
        // left click (and both `WM_RBUTTONUP` and `WM_CONTEXTMENU` on one right click).
        let event = (lparam.0 as u32) & 0xFFFF;
        match event {
            NIN_SELECT | NIN_KEYSELECT => {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                self.toggle_flyout(Some(pt));
            }
            WM_CONTEXTMENU => {
                self.hide_flyout();
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                self.refresh_theme_if_needed();
                let icon_mode = self.manager.tray_icon_mode();
                let autostart = is_autostart_enabled();
                let has_disabled = self.manager.has_disabled_profiles();
                if let Some(action) = self.tray.show_context_menu(
                    pt,
                    self.theme.app_dark,
                    icon_mode,
                    autostart,
                    has_disabled,
                ) {
                    self.handle_menu_action(action);
                }
            }
            _ => {}
        }
    }

    fn handle_menu_action(&mut self, action: TrayMenuAction) {
        match action {
            TrayMenuAction::OpenFlyout => self.open_flyout(None),
            TrayMenuAction::ToggleTrayIconMode => {
                let next = match self.manager.tray_icon_mode() {
                    TrayIconMode::VectorIcon => TrayIconMode::KeycapCode,
                    TrayIconMode::KeycapCode => TrayIconMode::VectorIcon,
                };
                let _ = self.manager.set_tray_icon_mode(next);
                self.update_tray_badge();
            }
            TrayMenuAction::ToggleAutostart => {
                let _ = set_autostart_enabled(!is_autostart_enabled());
            }
            TrayMenuAction::OpenSettings => {
                let _ = open_language_settings();
            }
            TrayMenuAction::RestoreAll => self.restore_all_layouts_async(),
            TrayMenuAction::RestoreAndExit => {
                // Drain any in-flight async worker plan first so restore never races the worker.
                self.worker.shutdown();
                self.suppress_reg_until = Some(Instant::now() + SELF_MUTATION_SUPPRESS);
                let _ = self.manager.restore_all();
                unsafe { PostQuitMessage(0) };
            }
            TrayMenuAction::Exit => {
                unsafe { PostQuitMessage(0) };
            }
        }
    }

    fn handle_click(&mut self, target: HitTarget) {
        match target {
            HitTarget::RowToggle(idx) => self.toggle_row(idx),
            HitTarget::RowActivate(idx) => self.activate_row(idx),
            HitTarget::FooterSettings => {
                self.hide_flyout();
                let _ = open_language_settings();
            }
            HitTarget::FooterRestoreAll => self.restore_all_layouts_async(),
            HitTarget::None => {}
        }
    }

    fn toggle_row(&mut self, idx: usize) {
        let Some(tip_id) = self
            .manager
            .state()
            .profiles
            .get(idx)
            .map(|p| p.tip_id.clone())
        else {
            return;
        };

        // Optimistic domain update (< 0.1 ms) -> smooth immediate animation on UI thread,
        // while `InstallLayoutOrTip` runs asynchronously on the dedicated STA worker thread.
        match self.manager.plan_toggle(&tip_id, self.active_index) {
            Ok((_updated, plan)) => {
                self.clear_notice();
                self.suppress_reg_until = Some(Instant::now() + SELF_MUTATION_SUPPRESS);
                self.ignore_deactivate_until = Some(Instant::now() + Duration::from_millis(400));

                if let Some(ref next_active) = plan.restore_active_after {
                    self.active_index = self
                        .manager
                        .state()
                        .profiles
                        .iter()
                        .position(|p| p.tip_id.eq_ignore_ascii_case(&next_active.tip_id));
                }
                self.update_tray_badge();
                self.worker.submit(plan);
                self.request_vsync_frame();
            }
            Err(AppError::CannotDisableLastLayout) => {
                self.show_notice("At least one active layout is required");
            }
            Err(e) => {
                self.show_notice(format!("Failed to update layout: {e}"));
            }
        }
    }

    fn activate_row(&mut self, idx: usize) {
        let Some(profile) = self.manager.state().profiles.get(idx).cloned() else {
            return;
        };
        if !profile.enabled {
            self.show_notice("Turn on the switch on the right first");
            return;
        }

        let live_hkl = self.manager.backend().find_live_hkl(&profile);
        let prev_fg = self.prev_foreground;
        self.hide_flyout();

        let _ = self.manager.activate(&profile.tip_id);
        notify_window_layout_change(prev_fg, live_hkl);

        self.active_index = Some(idx);
        self.last_polled_hkl = live_hkl;
        self.update_tray_badge();
    }

    fn restore_all_layouts_async(&mut self) {
        let Some(plan) = self.manager.plan_restore_all(self.active_index) else {
            return;
        };
        self.clear_notice();
        self.suppress_reg_until = Some(Instant::now() + SELF_MUTATION_SUPPRESS);
        self.ignore_deactivate_until = Some(Instant::now() + Duration::from_millis(400));
        self.worker.submit(plan);
        self.update_tray_badge();
        self.request_vsync_frame();
    }

    fn on_apply_complete(&mut self) {
        for outcome in self.worker.drain_outcomes() {
            match outcome {
                ApplyOutcome::Succeeded => {
                    self.suppress_reg_until = Some(Instant::now() + Duration::from_millis(450));
                    if self.visible {
                        unsafe {
                            let _ = SetForegroundWindow(self.hwnd);
                        }
                    }
                    self.active_index = resolve_active_index(&self.manager, foreground_hkl());
                    self.update_tray_badge();
                    self.request_vsync_frame();
                }
                ApplyOutcome::FailedRolledBack {
                    prev_profiles,
                    error,
                } => {
                    self.manager.revert_plan(prev_profiles);
                    self.active_index = resolve_active_index(&self.manager, foreground_hkl());
                    self.update_tray_badge();
                    self.show_notice(format!("Failed to update layout: {error}"));
                }
            }
        }
    }

    /// Displays a transient footer notice using a one-shot Win32 timer (0% idle CPU while shown).
    fn show_notice(&mut self, text: impl Into<String>) {
        self.status_notice = Some(text.into());
        // SAFETY: Arming a one-shot timer on our owned window to clear the notice automatically.
        unsafe {
            let _ = SetTimer(self.hwnd, TIMER_NOTICE_EXPIRE, NOTICE_DURATION_MS, None);
        }
        if self.visible {
            self.render_now();
        }
    }

    fn clear_notice(&mut self) {
        if self.status_notice.take().is_some() {
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_NOTICE_EXPIRE);
            }
        }
    }

    fn on_notice_expired(&mut self) {
        self.clear_notice();
        if self.visible {
            self.render_now();
        }
    }

    fn request_vsync_frame(&mut self) {
        self.animator.request_frame(Instant::now());
    }

    fn step_vsync_frame(&mut self) {
        let now = Instant::now();
        self.animator.step(
            now,
            &self.manager.state().profiles,
            self.interaction.hovered,
            self.interaction.pressed,
        );
        if self.visible {
            self.render_to_dc(None, now);
        }
    }

    fn on_active_poll_tick(&mut self) {
        let fg_hkl = foreground_hkl();
        if fg_hkl == self.last_polled_hkl {
            return;
        }
        self.last_polled_hkl = fg_hkl;

        let new_active = resolve_active_index(&self.manager, fg_hkl);
        if new_active != self.active_index {
            self.active_index = new_active;
            self.update_tray_badge();
            if self.visible {
                self.render_now();
            }
        }
    }

    fn on_registry_sync_debounced(&mut self) {
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_REG_DEBOUNCE);
        }
        if self
            .suppress_reg_until
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }

        if self.manager.sync_with_system().is_ok() {
            self.animator.sync_profiles(&self.manager.state().profiles);
            self.active_index = resolve_active_index(&self.manager, foreground_hkl());
            self.update_tray_badge();
            self.resize_if_visible();
            if self.visible {
                self.request_vsync_frame();
            }
        }
    }

    fn on_setting_change(&mut self) {
        if self.refresh_theme_if_needed() && self.visible {
            self.render_now();
        }
    }

    fn refresh_theme_if_needed(&mut self) -> bool {
        let fresh = SystemTheme::read();
        if fresh == self.theme {
            return false;
        }
        self.theme = fresh;
        self.palette = FluentPalette::from_theme(fresh);
        apply_flyout_dwm(self.hwnd, fresh.app_dark, fresh.transparency_enabled);
        self.update_tray_badge();
        true
    }

    fn update_tray_badge(&mut self) {
        let code = self.manager.keycap_code_for(self.active_index);
        let mode = self.manager.tray_icon_mode();
        let _ = self
            .tray
            .update_icon(mode, &code, self.theme.taskbar_dark, self.dpi);
    }

    fn render_now(&mut self) {
        self.render_to_dc(None, Instant::now());
    }

    fn render_to_dc(&mut self, paint_dc: Option<HDC>, now: Instant) {
        let geom = self.current_geometry();
        let enabled_count = self.manager.enabled_count();
        let open_progress = self.animator.open_progress(now);

        let rows: Vec<RowRenderItem<'_>> = self
            .manager
            .state()
            .profiles
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let anim = self.animator.row_anim(p);
                RowRenderItem {
                    badge: &p.short_badge,
                    language_name: &p.language_name,
                    layout_name: &p.layout_name,
                    is_active: self.active_index == Some(idx),
                    is_enabled: p.enabled,
                    is_locked_last: p.enabled && enabled_count <= 1,
                    toggle_t: anim.toggle_t,
                    row_hover_t: anim.row_hover_t,
                    knob_hover_t: anim.knob_hover_t,
                    knob_press_t: anim.knob_press_t,
                }
            })
            .collect();

        let view = FlyoutRenderView {
            geometry: &geom,
            palette: &self.palette,
            rows: &rows,
            pressed: self.interaction.pressed,
            settings_hover_t: self.animator.settings_hover_t(),
            restore_hover_t: self.animator.restore_hover_t(),
            focused_row: self.interaction.focused_row,
            status_notice: self.status_notice.as_deref(),
            open_progress,
        };
        let _ = self.renderer.render(&view, paint_dc);
    }

    fn on_mouse_move(&mut self, lparam: LPARAM) {
        if !self.tracking_mouse {
            self.tracking_mouse = true;
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
        }

        let target = self.hit_test_lparam(lparam);
        if self.interaction.on_hover(target) {
            self.request_vsync_frame();
        }
    }

    fn on_mouse_leave(&mut self) {
        self.tracking_mouse = false;
        if self.interaction.on_mouse_leave() {
            self.request_vsync_frame();
        }
    }

    fn on_key_down(&mut self, vk: u16) {
        let count = self.manager.state().profiles.len();
        if count == 0 {
            return;
        }
        match vk {
            x if x == VK_ESCAPE.0 => self.hide_flyout(),
            x if x == VK_DOWN.0 => {
                if self.interaction.move_focus_down(count, self.active_index) {
                    self.render_now();
                }
            }
            x if x == VK_UP.0 => {
                if self.interaction.move_focus_up(count, self.active_index) {
                    self.render_now();
                }
            }
            x if x == VK_SPACE.0 => {
                if let Some(i) = self.interaction.selected_row(self.active_index) {
                    self.toggle_row(i);
                }
            }
            x if x == VK_RETURN.0 => {
                if let Some(i) = self.interaction.selected_row(self.active_index) {
                    self.activate_row(i);
                }
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Retrieving the `RefCell<FlyoutApp>` pointer stored in `GWLP_USERDATA` on the GUI thread.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut RefCell<FlyoutApp>;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    let Ok(mut app) = (unsafe { &*ptr }).try_borrow_mut() else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };

    if msg == app.wake_message && app.wake_message != 0 {
        app.open_flyout(None);
        return LRESULT(0);
    }

    if msg == app.taskbar_created_msg && app.taskbar_created_msg != 0 {
        let code = app.manager.keycap_code_for(app.active_index);
        let mode = app.manager.tray_icon_mode();
        let taskbar_dark = app.theme.taskbar_dark;
        let dpi = app.dpi;
        let _ = app.tray.add_or_recreate(mode, &code, taskbar_dark, dpi);
        return LRESULT(0);
    }

    match msg {
        WM_TRAY_CALLBACK => {
            app.handle_tray_event(lparam);
            LRESULT(0)
        }
        WM_APPLY_COMPLETE => {
            app.on_apply_complete();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: Paired `BeginPaint`/`EndPaint` for `WM_PAINT`.
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if app.visible {
                app.render_to_dc(Some(hdc), Instant::now());
            }
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let state = (wparam.0 as u32) & 0xFFFF;
            if state == WA_INACTIVE {
                let guarded = app
                    .ignore_deactivate_until
                    .is_some_and(|until| Instant::now() < until);
                if !guarded {
                    app.hide_flyout();
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            app.on_mouse_move(lparam);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            app.on_mouse_leave();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let target = app.hit_test_lparam(lparam);
            app.interaction.on_press(target);
            app.request_vsync_frame();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let target = app.hit_test_lparam(lparam);
            if let Some(clicked) = app.interaction.on_release(target) {
                app.handle_click(clicked);
            } else if app.visible {
                app.request_vsync_frame();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) as i16) as f32 / 120.0;
            let max_scroll = app.current_geometry().max_scroll;
            if app.interaction.on_scroll(delta, max_scroll) {
                app.render_now();
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let cursor_id = if app.interaction.hovered == HitTarget::None {
                IDC_ARROW
            } else {
                IDC_HAND
            };
            unsafe {
                if let Ok(cur) = LoadCursorW(None, cursor_id) {
                    SetCursor(cur);
                }
            }
            LRESULT(1)
        }
        WM_KEYDOWN => {
            app.on_key_down(wparam.0 as u16);
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                TIMER_ACTIVE_POLL => app.on_active_poll_tick(),
                TIMER_REG_DEBOUNCE => app.on_registry_sync_debounced(),
                TIMER_NOTICE_EXPIRE => app.on_notice_expired(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            app.on_setting_change();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let new_dpi = ((wparam.0 >> 16) as u32).max(96);
            app.dpi = new_dpi;
            app.update_tray_badge();
            app.resize_if_visible();
            if app.visible {
                app.render_now();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn resolve_active_index(mgr: &SystemLayoutManager, fg_hkl: Option<isize>) -> Option<usize> {
    if let Some(hkl) = fg_hkl
        && let Some(idx) = mgr.backend().match_hkl_to_index(hkl, &mgr.state().profiles)
    {
        return Some(idx);
    }
    mgr.active_profile_index()
        .or_else(|| mgr.state().profiles.iter().position(|p| p.enabled))
}
