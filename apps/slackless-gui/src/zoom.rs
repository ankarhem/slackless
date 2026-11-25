//! Slack's zoom (Preferences > Accessibility, Cmd+= / Cmd+- / Cmd+0): the whole UI scaled
//! uniformly, as Electron's zoom factor scales Slack's. GPUI has no zoom, so it is applied at
//! the window boundary, the way a browser zooms: the platform is wrapped so each window
//! reports its display scale times the zoom and its content size (and every input position)
//! divided by it. Layout then runs in a smaller logical viewport rendered at a higher pixel
//! density, so `px` sizes, rems and gpui-kit's own components all scale with no call site
//! aware of it. Window bounds, resizing and the traffic lights stay native.
//!
//! Known limits: macOS places system text-input popups (the emoji & symbols picker,
//! dictation) and the native context menu of text fields from unscaled positions, so away from
//! 100% they open offset from the caret.
//!
//! `Zoom::platform(&self) -> Rc<dyn Platform>` (`Application::with_platform`), the `Zoom`
//! global, `set_level(usize, &mut App) -> bool`, `FACTORS`, `ACTUAL_SIZE`.

use std::cell::Cell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

static STRIP_TRACKING_PARAMS: AtomicBool = AtomicBool::new(true);

/// Enable or disable automatic stripping of tracking query parameters on every `open_url` call.
pub fn set_strip_tracking_params(enabled: bool) {
    STRIP_TRACKING_PARAMS.store(enabled, Ordering::Relaxed);
}

use anyhow::Result;
use futures::channel::oneshot;
use gpui_kit::*;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use smallvec::SmallVec;

/// Preferences' zoom levels (its `80%` to `150%` choices) as factors.
pub const FACTORS: [f32; 6] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5];

/// The level of [`FACTORS`] that is 100% (Cmd+0, Actual Size).
pub const ACTUAL_SIZE: usize = 2;

/// The zoom factor every window reads.
type Factor = Rc<Cell<f32>>;

/// The zoom shared by the platform wrapper and the app (a GPUI global).
#[derive(Clone)]
pub struct Zoom(Factor);

impl Default for Zoom {
    fn default() -> Self {
        Self(Rc::new(Cell::new(1.0)))
    }
}

impl Global for Zoom {}

impl Zoom {
    /// The factor applied now.
    pub fn factor(&self) -> f32 {
        self.0.get()
    }

    /// The current platform with this zoom applied to every window it opens.
    pub fn platform(&self) -> Rc<dyn Platform> {
        Rc::new(ZoomPlatform {
            inner: platform::current_platform(false),
            zoom: self.0.clone(),
        })
    }
}

/// Zoom every window to Preferences' level `level`; whether that changed the zoom. The
/// windows re-read their size and scale once the current update is done: a window cannot
/// re-lay out (and run its bounds observers) inside an update of its own views.
pub fn set_level(level: usize, cx: &mut App) -> bool {
    let factor = FACTORS[level];
    let zoom = cx.global::<Zoom>().0.clone();
    if zoom.get() == factor {
        return false;
    }
    zoom.set(factor);
    for window in cx.windows() {
        cx.defer(move |cx| {
            // Fails only for a window closed meanwhile, which has nothing to re-lay out.
            let _ = window.update(cx, |_, window, cx| window.bounds_changed(cx));
        });
    }
    true
}

struct ZoomPlatform {
    inner: Rc<dyn Platform>,
    zoom: Factor,
}

struct ZoomWindow {
    inner: Box<dyn PlatformWindow>,
    zoom: Factor,
    restoring_state: Rc<Cell<bool>>,
}

/// Map an input event from native points into the zoomed logical space.
fn unzoom(event: PlatformInput, z: f32) -> PlatformInput {
    if z == 1.0 {
        return event;
    }
    let p = |p: Point<Pixels>| p.map(|v| v / z);
    match event {
        PlatformInput::MouseDown(mut e) => {
            e.position = p(e.position);
            PlatformInput::MouseDown(e)
        }
        PlatformInput::MouseUp(mut e) => {
            e.position = p(e.position);
            PlatformInput::MouseUp(e)
        }
        PlatformInput::MouseMove(mut e) => {
            e.position = p(e.position);
            PlatformInput::MouseMove(e)
        }
        PlatformInput::MousePressure(mut e) => {
            e.position = p(e.position);
            PlatformInput::MousePressure(e)
        }
        PlatformInput::MouseExited(mut e) => {
            e.position = p(e.position);
            PlatformInput::MouseExited(e)
        }
        PlatformInput::Scroll(mut e) => {
            e.position = p(e.position);
            if let ScrollDelta::Pixels(delta) = e.delta {
                e.delta = ScrollDelta::Pixels(p(delta));
            }
            PlatformInput::Scroll(e)
        }
        PlatformInput::Pinch(mut e) => {
            e.position = p(e.position);
            PlatformInput::Pinch(e)
        }
        PlatformInput::FileDrop(e) => PlatformInput::FileDrop(match e {
            FileDropEvent::Entered { position, paths } => FileDropEvent::Entered {
                position: p(position),
                paths,
            },
            FileDropEvent::Pending { position } => FileDropEvent::Pending {
                position: p(position),
            },
            FileDropEvent::Submit { position } => FileDropEvent::Submit {
                position: p(position),
            },
            other => other,
        }),
        other => other,
    }
}

// Every method forwarded, so a GPUI update that adds one fails clippy here. Test builds add
// GPUI's test-support methods, which keep their defaults: no test opens a zoomed window.
#[cfg_attr(not(test), deny(clippy::missing_trait_methods))]
impl Platform for ZoomPlatform {
    fn platform_info(&self) -> PlatformInfo {
        self.inner.platform_info()
    }
    fn background_executor(&self) -> BackgroundExecutor {
        self.inner.background_executor()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.inner.foreground_executor()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.inner.text_system()
    }
    fn run(&self, a0: Box<dyn 'static + FnOnce()>) {
        self.inner.run(a0)
    }
    fn quit(&self) {
        self.inner.quit()
    }
    fn set_initial_windowing(&self, a0: WindowingRequest) {
        self.inner.set_initial_windowing(a0)
    }
    fn request_windowing(&self, a0: WindowingRequest) -> Task<anyhow::Result<()>> {
        self.inner.request_windowing(a0)
    }
    fn restart(&self, a0: Option<PathBuf>, a1: Vec<OsString>) {
        self.inner.restart(a0, a1)
    }
    fn activate(&self, a0: bool) {
        self.inner.activate(a0)
    }
    fn set_activation_policy(&self, a0: ActivationPolicy) {
        self.inner.set_activation_policy(a0)
    }
    fn hide(&self) {
        self.inner.hide()
    }
    fn hide_other_apps(&self) {
        self.inner.hide_other_apps()
    }
    fn unhide_other_apps(&self) {
        self.inner.unhide_other_apps()
    }
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.inner.displays()
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.inner.primary_display()
    }
    fn on_displays_changed(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_displays_changed(a0)
    }
    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.inner.active_window()
    }
    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        self.inner.window_stack()
    }
    fn is_screen_capture_supported(&self) -> bool {
        self.inner.is_screen_capture_supported()
    }
    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<anyhow::Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        self.inner.screen_capture_sources()
    }
    fn open_window(
        &self,
        a0: AnyWindowHandle,
        a1: WindowParams,
    ) -> anyhow::Result<Box<dyn PlatformWindow>> {
        let inner = self.inner.open_window(a0, a1)?;
        Ok(Box::new(ZoomWindow {
            inner,
            zoom: self.zoom.clone(),
            restoring_state: Rc::new(Cell::new(false)),
        }))
    }
    fn window_appearance(&self) -> WindowAppearance {
        self.inner.window_appearance()
    }
    fn set_window_appearance(&self, a0: Option<WindowAppearance>) {
        self.inner.set_window_appearance(a0)
    }
    fn button_layout(&self) -> Option<WindowButtonLayout> {
        self.inner.button_layout()
    }
    fn open_url(&self, a0: &str) {
        if STRIP_TRACKING_PARAMS.load(Ordering::Relaxed) {
            let cleaned = slackless_core::strip_tracking_params(a0);
            self.inner.open_url(&cleaned);
        } else {
            self.inner.open_url(a0);
        }
    }
    fn on_open_urls(&self, a0: Box<dyn FnMut(Vec<String>)>) {
        self.inner.on_open_urls(a0)
    }
    fn register_url_scheme(&self, a0: &str) -> Task<Result<()>> {
        self.inner.register_url_scheme(a0)
    }
    fn prompt_for_paths(
        &self,
        a0: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.inner.prompt_for_paths(a0)
    }
    fn prompt_for_new_path(
        &self,
        a0: &Path,
        a1: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        self.inner.prompt_for_new_path(a0, a1)
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        self.inner.can_select_mixed_files_and_dirs()
    }
    fn reveal_path(&self, a0: &Path) {
        self.inner.reveal_path(a0)
    }
    fn open_with_system(&self, a0: &Path) {
        self.inner.open_with_system(a0)
    }
    fn on_quit(&self, a0: Box<dyn FnMut() -> bool>) {
        self.inner.on_quit(a0)
    }
    fn on_reopen(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_reopen(a0)
    }
    fn on_system_sleep(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_system_sleep(a0)
    }
    fn on_system_wake(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_system_wake(a0)
    }
    fn on_app_lifecycle(&self, a0: Box<dyn FnMut(AppLifecyclePhase)>) {
        self.inner.on_app_lifecycle(a0)
    }
    fn on_memory_warning(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_memory_warning(a0)
    }
    fn gestures(&self) -> Option<Rc<dyn PlatformGestures>> {
        self.inner.gestures()
    }
    fn set_menus(&self, a0: Vec<Menu>, a1: &Keymap) {
        self.inner.set_menus(a0, a1)
    }
    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        self.inner.get_menus()
    }
    fn set_dock_menu(&self, a0: Vec<MenuItem>, a1: &Keymap) {
        self.inner.set_dock_menu(a0, a1)
    }
    fn perform_dock_menu_action(&self, a0: usize) {
        self.inner.perform_dock_menu_action(a0)
    }
    fn add_recent_document(&self, a0: &Path) {
        self.inner.add_recent_document(a0)
    }
    fn update_jump_list(
        &self,
        a0: Vec<MenuItem>,
        a1: Vec<SmallVec<[PathBuf; 2]>>,
    ) -> Task<Vec<SmallVec<[PathBuf; 2]>>> {
        self.inner.update_jump_list(a0, a1)
    }
    fn on_app_menu_action(&self, a0: Box<dyn FnMut(&dyn Action)>) {
        self.inner.on_app_menu_action(a0)
    }
    fn on_will_open_app_menu(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_will_open_app_menu(a0)
    }
    fn on_validate_app_menu_command(&self, a0: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.inner.on_validate_app_menu_command(a0)
    }
    fn thermal_state(&self) -> ThermalState {
        self.inner.thermal_state()
    }
    fn on_thermal_state_change(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_thermal_state_change(a0)
    }
    fn prevent_idle_sleep(&self, a0: &str) -> Task<Result<ActivityGuard>> {
        self.inner.prevent_idle_sleep(a0)
    }
    fn power_mode(&self) -> PowerMode {
        self.inner.power_mode()
    }
    fn on_power_mode_change(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_power_mode_change(a0)
    }
    fn set_app_identity(&self, a0: &str, a1: &str) {
        self.inner.set_app_identity(a0, a1)
    }
    fn show_system_notification(&self, a0: SystemNotification) {
        self.inner.show_system_notification(a0)
    }
    fn dismiss_system_notification(&self, a0: &str) -> gpui::Task<()> {
        self.inner.dismiss_system_notification(a0)
    }
    fn on_system_notification_response(&self, a0: Box<dyn FnMut(SystemNotificationResponse)>) {
        self.inner.on_system_notification_response(a0)
    }
    fn compositor_name(&self) -> &'static str {
        self.inner.compositor_name()
    }
    fn graphical_environment(&self) -> Option<GraphicalEnvironment> {
        self.inner.graphical_environment()
    }
    fn debug_state(&self) -> crate::blockkit::serde_json::Value {
        self.inner.debug_state()
    }
    fn app_path(&self) -> Result<PathBuf> {
        self.inner.app_path()
    }
    fn path_for_auxiliary_executable(&self, a0: &str) -> Result<PathBuf> {
        self.inner.path_for_auxiliary_executable(a0)
    }
    fn set_cursor_style(&self, a0: CursorStyle) {
        self.inner.set_cursor_style(a0)
    }
    fn hide_cursor_until_mouse_moves(&self) {
        self.inner.hide_cursor_until_mouse_moves()
    }
    fn is_cursor_visible(&self) -> bool {
        self.inner.is_cursor_visible()
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        self.inner.should_auto_hide_scrollbars()
    }
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.inner.read_from_clipboard()
    }
    fn write_to_clipboard(&self, a0: ClipboardItem) {
        self.inner.write_to_clipboard(a0)
    }
    fn read_from_clipboard_async(&self) -> Task<Result<Option<ClipboardItem>, ClipboardReadError>> {
        self.inner.read_from_clipboard_async()
    }
    #[cfg(target_os = "macos")]
    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        self.inner.read_from_find_pasteboard()
    }
    #[cfg(target_os = "macos")]
    fn write_to_find_pasteboard(&self, a0: ClipboardItem) {
        self.inner.write_to_find_pasteboard(a0)
    }
    fn write_credentials(&self, a0: &str, a1: &str, a2: &[u8]) -> Task<Result<()>> {
        self.inner.write_credentials(a0, a1, a2)
    }
    #[cfg(target_os = "macos")]
    fn write_credentials_if_not_exists(&self, a0: &str, a1: &str, a2: &[u8]) -> Task<Result<bool>> {
        self.inner.write_credentials_if_not_exists(a0, a1, a2)
    }
    fn read_credentials(&self, a0: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        self.inner.read_credentials(a0)
    }
    fn delete_credentials(&self, a0: &str) -> Task<Result<()>> {
        self.inner.delete_credentials(a0)
    }
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        self.inner.keyboard_layout()
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        self.inner.keyboard_mapper()
    }
    fn on_keyboard_layout_change(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_keyboard_layout_change(a0)
    }
}

impl HasWindowHandle for ZoomWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for ZoomWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.inner.display_handle()
    }
}

#[cfg_attr(not(test), deny(clippy::missing_trait_methods))]
impl PlatformWindow for ZoomWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.inner.bounds()
    }
    fn is_maximized(&self) -> bool {
        self.inner.is_maximized()
    }
    fn window_bounds(&self) -> WindowBounds {
        self.inner.window_bounds()
    }
    fn content_size(&self) -> Size<Pixels> {
        let z = self.zoom.get();
        self.inner.content_size().map(|v| v / z)
    }
    fn visual_viewport_bounds(&self) -> Bounds<Pixels> {
        let z = self.zoom.get();
        let b = self.inner.visual_viewport_bounds();
        Bounds::new(b.origin.map(|v| v / z), b.size.map(|v| v / z))
    }
    fn on_visual_viewport_changed(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_visual_viewport_changed(a0)
    }
    fn prepare_frame(&self) -> bool {
        self.inner.prepare_frame()
    }
    fn resize(&mut self, a0: Size<Pixels>) {
        self.inner.resize(a0)
    }
    fn scale_factor(&self) -> f32 {
        self.inner.scale_factor() * self.zoom.get()
    }
    fn appearance(&self) -> WindowAppearance {
        self.inner.appearance()
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.inner.display()
    }
    fn mouse_position(&self) -> Point<Pixels> {
        let z = self.zoom.get();
        self.inner.mouse_position().map(|v| v / z)
    }
    fn modifiers(&self) -> Modifiers {
        self.inner.modifiers()
    }
    fn capslock(&self) -> Capslock {
        self.inner.capslock()
    }
    fn set_input_handler(&mut self, a0: PlatformInputHandler) {
        self.inner.set_input_handler(a0)
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.inner.take_input_handler()
    }
    fn set_text_input_configuration(&mut self, a0: TextInputConfiguration) {
        self.inner.set_text_input_configuration(a0)
    }
    fn prompt(
        &self,
        a0: PromptLevel,
        a1: &str,
        a2: Option<&str>,
        a3: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        self.inner.prompt(a0, a1, a2, a3)
    }
    fn activate(&self) {
        self.inner.activate()
    }
    fn request_attention(&self) {
        self.inner.request_attention()
    }
    fn is_active(&self) -> bool {
        self.inner.is_active()
    }
    fn visibility(&self) -> WindowVisibility {
        self.inner.visibility()
    }
    fn is_hovered(&self) -> bool {
        self.inner.is_hovered()
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.inner.background_appearance()
    }
    fn set_title(&mut self, a0: &str) {
        self.inner.set_title(a0)
    }
    fn set_background_appearance(&self, a0: WindowBackgroundAppearance) {
        self.inner.set_background_appearance(a0)
    }
    fn minimize(&self) {
        self.inner.minimize()
    }
    fn zoom(&self) {
        self.inner.zoom()
    }
    fn toggle_fullscreen(&self) {
        self.inner.toggle_fullscreen()
    }
    fn is_fullscreen(&self) -> bool {
        self.inner.is_fullscreen()
    }
    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        self.inner.frame_waker()
    }
    fn on_request_frame(&self, a0: Box<dyn FnMut(RequestFrameOptions)>) {
        self.inner.on_request_frame(a0)
    }
    fn on_input(&self, a0: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        let zoom = self.zoom.clone();
        let mut callback = a0;
        self.inner
            .on_input(Box::new(move |event| callback(unzoom(event, zoom.get()))))
    }
    fn on_active_status_change(&self, a0: Box<dyn FnMut(bool)>) {
        self.inner.on_active_status_change(a0)
    }
    fn on_visibility_change(&self, a0: Box<dyn FnMut(WindowVisibility)>) {
        self.inner.on_visibility_change(a0)
    }
    fn on_hover_status_change(&self, a0: Box<dyn FnMut(bool)>) {
        self.inner.on_hover_status_change(a0)
    }
    fn on_resize(&self, a0: Box<dyn FnMut(Size<Pixels>, f32)>) {
        let zoom = self.zoom.clone();
        let restoring = self.restoring_state.clone();
        let mut callback = a0;
        self.inner.on_resize(Box::new(move |size, scale| {
            // Suppress synchronous AppKit `windowDidResize:` callbacks fired re-entrantly while
            // `restore_native_window_state` runs inside an active GPUI `App` borrow; the caller
            // follows restoration immediately with `window.bounds_changed(cx)`.
            if restoring.get() {
                return;
            }
            let z = zoom.get();
            callback(size.map(|v| v / z), scale * z)
        }))
    }
    fn on_moved(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_moved(a0)
    }
    fn on_display_changed(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_display_changed(a0)
    }
    fn on_should_close(&self, a0: Box<dyn FnMut() -> bool>) {
        self.inner.on_should_close(a0)
    }
    fn on_hit_test_window_control(&self, a0: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.inner.on_hit_test_window_control(a0)
    }
    fn on_close(&self, a0: Box<dyn FnOnce()>) {
        self.inner.on_close(a0)
    }
    fn on_appearance_changed(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_appearance_changed(a0)
    }
    fn on_button_layout_changed(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_button_layout_changed(a0)
    }
    fn draw(&self, a0: &Scene) {
        self.inner.draw(a0)
    }
    fn schedule_frame(&self) {
        self.inner.schedule_frame()
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.inner.sprite_atlas()
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        self.inner.is_subpixel_rendering_supported()
    }
    fn get_title(&self) -> String {
        self.inner.get_title()
    }
    fn tabbed_windows(&self) -> Option<Vec<SystemWindowTab>> {
        self.inner.tabbed_windows()
    }
    fn tab_bar_visible(&self) -> bool {
        self.inner.tab_bar_visible()
    }
    fn set_edited(&mut self, a0: bool) {
        self.inner.set_edited(a0)
    }
    fn set_document_path(&self, a0: Option<&std::path::Path>) {
        self.inner.set_document_path(a0)
    }
    fn toggle_simple_fullscreen(&self) {
        self.inner.toggle_simple_fullscreen()
    }
    fn is_simple_fullscreen(&self) -> bool {
        self.inner.is_simple_fullscreen()
    }
    #[cfg(target_os = "macos")]
    fn set_traffic_light_position(&self, a0: Point<Pixels>) {
        self.inner.set_traffic_light_position(a0)
    }
    fn show_character_palette(&self) {
        self.inner.show_character_palette()
    }
    fn titlebar_double_click(&self, a0: bool, a1: bool) {
        self.inner.titlebar_double_click(a0, a1)
    }
    fn on_move_tab_to_new_window(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_move_tab_to_new_window(a0)
    }
    fn on_merge_all_windows(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_merge_all_windows(a0)
    }
    fn on_select_previous_tab(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_select_previous_tab(a0)
    }
    fn on_select_next_tab(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_select_next_tab(a0)
    }
    fn on_toggle_tab_bar(&self, a0: Box<dyn FnMut()>) {
        self.inner.on_toggle_tab_bar(a0)
    }
    fn merge_all_windows(&self) {
        self.inner.merge_all_windows()
    }
    fn move_tab_to_new_window(&self) {
        self.inner.move_tab_to_new_window()
    }
    fn toggle_window_tab_overview(&self) {
        self.inner.toggle_window_tab_overview()
    }
    fn set_tabbing_identifier(&self, a0: Option<String>) {
        self.inner.set_tabbing_identifier(a0)
    }
    fn native_window_state(&self) -> Option<Vec<u8>> {
        self.inner.native_window_state()
    }
    fn restore_native_window_state(&self, a0: &[u8]) {
        self.restoring_state.set(true);
        self.inner.restore_native_window_state(a0);
        self.restoring_state.set(false);
    }
    fn inner_window_bounds(&self) -> WindowBounds {
        self.inner.inner_window_bounds()
    }
    fn request_decorations(&self, a0: WindowDecorations) {
        self.inner.request_decorations(a0)
    }
    fn show_window_menu(&self, a0: Point<Pixels>) {
        self.inner.show_window_menu(a0)
    }
    fn start_window_move(&self) {
        self.inner.start_window_move()
    }
    fn can_start_external_drag(&self) -> bool {
        self.inner.can_start_external_drag()
    }
    fn start_external_drag(&self, a0: &ExternalDragPayload) -> bool {
        self.inner.start_external_drag(a0)
    }
    fn start_window_resize(&self, a0: ResizeEdge) {
        self.inner.start_window_resize(a0)
    }
    fn set_exclusive_zone(&self, a0: Pixels) {
        self.inner.set_exclusive_zone(a0)
    }
    fn set_input_region(&self, a0: Option<&[Bounds<Pixels>]>) {
        self.inner.set_input_region(a0)
    }
    fn window_decorations(&self) -> Decorations {
        self.inner.window_decorations()
    }
    fn set_app_id(&mut self, a0: &str) {
        self.inner.set_app_id(a0)
    }
    fn map_window(&mut self) -> anyhow::Result<()> {
        self.inner.map_window()
    }
    fn window_controls(&self) -> WindowControls {
        self.inner.window_controls()
    }
    fn set_client_inset(&self, a0: Pixels) {
        self.inner.set_client_inset(a0)
    }
    fn set_client_corner_radii(&self, a0: Corners<Pixels>) {
        self.inner.set_client_corner_radii(a0)
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.inner.gpu_specs()
    }
    fn update_ime_position(&self, a0: Bounds<Pixels>) {
        let z = self.zoom.get();
        self.inner.update_ime_position(Bounds::new(
            a0.origin.map(|v| v * z),
            a0.size.map(|v| v * z),
        ))
    }
    fn insets(&self) -> WindowInsets {
        self.inner.insets()
    }
    fn on_insets_changed(&self, a0: Box<dyn FnMut(WindowInsets)>) {
        self.inner.on_insets_changed(a0)
    }
    fn set_back_handler(&self, a0: Box<dyn FnMut()>) {
        self.inner.set_back_handler(a0)
    }
    fn set_back_enabled(&self, a0: bool) {
        self.inner.set_back_enabled(a0)
    }
    fn show_soft_keyboard(&self) {
        self.inner.show_soft_keyboard()
    }
    fn hide_soft_keyboard(&self) {
        self.inner.hide_soft_keyboard()
    }
    fn text_input_state_changed(&self, a0: TextInputStateChange) {
        self.inner.text_input_state_changed(a0)
    }
    fn play_system_bell(&self) {
        self.inner.play_system_bell()
    }
    fn a11y_init(&self, a0: A11yCallbacks) {
        self.inner.a11y_init(a0)
    }
    fn a11y_tree_update(&self, a0: accesskit::TreeUpdate) {
        self.inner.a11y_tree_update(a0)
    }
    fn a11y_update_window_bounds(&self) {
        self.inner.a11y_update_window_bounds()
    }
    fn debug_state(&self) -> crate::blockkit::serde_json::Value {
        self.inner.debug_state()
    }
    unsafe fn attach_native_view(
        &self,
        id: gpui::NativeViewId,
        view: std::ptr::NonNull<std::ffi::c_void>,
    ) -> Option<Box<dyn std::any::Any>> {
        // SAFETY: forwarded directly to the wrapped `PlatformWindow` under the caller's contract.
        unsafe { self.inner.attach_native_view(id, view) }
    }
    fn set_focused_native_view(&self, id: Option<gpui::NativeViewId>) {
        self.inner.set_focused_native_view(id)
    }
    fn on_hit_test_native_view(
        &self,
        callback: Box<dyn FnMut(Point<Pixels>) -> Option<gpui::NativeViewId>>,
    ) {
        self.inner.on_hit_test_native_view(callback)
    }
    fn on_native_view_focused(&self, callback: Box<dyn FnMut(gpui::NativeViewId)>) {
        self.inner.on_native_view_focused(callback)
    }
    fn on_key_down_before_native_view(&self, callback: Box<dyn FnMut(gpui::KeyDownEvent) -> bool>) {
        self.inner.on_key_down_before_native_view(callback)
    }
    fn set_native_titlebar_visibility(&self, visible: bool) {
        self.inner.set_native_titlebar_visibility(visible)
    }
}
