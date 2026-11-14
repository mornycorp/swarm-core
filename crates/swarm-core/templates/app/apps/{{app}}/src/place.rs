//! The window's size and place between launches (the App UI Substrate › The window).
//!
//! The app keeps them itself, not eframe (`persist_window: false`): a window closed while
//! minimized reports a parked corner and no size, which the next launch would open as a sliver. So
//! the geometry is checked at both ends:
//!
//! - **Saving** ([`observed`]): only a normal window's rectangle is kept. Minimized, maximized or full
//!   screen, the last normal one stays; maximized is remembered on its own.
//! - **Restoring** ([`placement`]): the saved rectangle is used only when at least half of it is on a
//!   connected monitor and it is larger than the minimum size; otherwise the window opens at the
//!   default size, centred on the primary monitor. The monitors are read when the event loop starts,
//!   before eframe opens the window ([`Launch`]).
//!
//! The file sits beside the UI state's (`--ui-state`, [`file_beside`]), and is read through the
//! backends' capped read and written through their one atomic writer, as hostile input: one too
//! large, unreadable or from another version is ignored and said so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::UserEvent;
use eframe::egui;
use egui::{Pos2, Rect, Vec2, pos2, vec2};
use {{crate_prefix_ident}}backends::files;
use {{crate_prefix_ident}}engine::lock::Lock;
use serde::{Deserialize, Serialize};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::WindowId;

/// The window's size at a first launch, or when the saved one can't be used, in points.
pub const DEFAULT_SIZE: Vec2 = vec2(1280.0, 800.0);
/// The smallest the window can be made, in points.
pub const MIN_SIZE: Vec2 = vec2(640.0, 400.0);
/// The version [`Saved`] is written at; a file at another is ignored.
const VERSION: u32 = 1;
/// The largest file read: a real one is about 150 bytes.
const MAX_FILE: u64 = 4096;
/// The share of the saved window that must be on one monitor for it to be used.
const MIN_ON_SCREEN: f32 = 0.5;
/// Windows parks a minimized window at (-32000, -32000): a corner this far out is never saved.
const MINIMIZED_CORNER: f32 = -30000.0;

/// What the app saves: its last normal window, in physical pixels, and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub version: u32,
    /// The window frame's top-left corner.
    pub outer: [f32; 2],
    /// The content area: left, top, width, height.
    pub inner: [f32; 4],
    /// egui's zoom when saved: a monitor's physical pixels per point are its scale times this.
    pub zoom: f32,
    pub maximized: bool,
}

impl Saved {
    /// The content area as a rectangle, if its numbers make one.
    fn inner_rect(&self) -> Option<Rect> {
        let [x, y, w, h] = self.inner;
        let rect = Rect::from_min_size(pos2(x, y), vec2(w, h));
        plausible(rect).then_some(rect)
    }
}

/// A connected monitor: its rectangle in physical pixels and its scale (pixels per point).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub rect: Rect,
    pub scale: f32,
    pub primary: bool,
}

/// Where and how the window opens. `position` (points, as egui's viewport takes it) is `None` when
/// no monitor is known: the system places the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub position: Option<Pos2>,
    pub size: Vec2,
    pub maximized: bool,
}

impl Placement {
    /// `builder` with this placement, replacing whatever was there.
    pub fn apply(self, mut builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        builder.position = self.position;
        builder.with_inner_size(self.size).with_maximized(self.maximized).with_fullscreen(false)
    }
}

/// Where the window opens: the saved window when it passes the checks (module docs), else the
/// default size centred on the primary monitor. A saved maximized state is kept either way.
pub fn placement(saved: Option<&Saved>, monitors: &[Monitor]) -> Placement {
    let zoom = saved.map_or(1.0, |s| sane_zoom(s.zoom));
    let fallback = || Placement { maximized: saved.is_some_and(|s| s.maximized), ..default_placement(monitors, zoom) };
    saved.and_then(|s| restored(s, monitors)).unwrap_or_else(fallback)
}

/// The saved window, if at least half of it is on one monitor and it is over the minimum size.
fn restored(saved: &Saved, monitors: &[Monitor]) -> Option<Placement> {
    let inner = saved.inner_rect()?;
    let monitor = home_monitor(inner, monitors)?;
    let per_point = monitor.scale * sane_zoom(saved.zoom);
    let size = inner.size() / per_point;
    if size.x <= MIN_SIZE.x || size.y <= MIN_SIZE.y {
        return None;
    }
    let corner = corner(saved, inner, cfg!(target_os = "macos"));
    let corner = corner.is_finite().then_some(corner)?;
    Some(Placement { position: Some(corner / per_point), size, maximized: saved.maximized })
}

/// The corner winit places, in pixels: the frame's (`saved.outer`), or the content's (`inner`'s)
/// where it places the content (`content`: macOS).
fn corner(saved: &Saved, inner: Rect, content: bool) -> Pos2 {
    if content { inner.min } else { pos2(saved.outer[0], saved.outer[1]) }
}

/// The monitor holding most of `rect`, if it holds at least half of it.
fn home_monitor(rect: Rect, monitors: &[Monitor]) -> Option<&Monitor> {
    let (monitor, overlap) = monitors.iter().map(|m| (m, m.rect.intersect(rect).area())).max_by(|a, b| a.1.total_cmp(&b.1))?;
    (overlap >= MIN_ON_SCREEN * rect.area()).then_some(monitor)
}

/// The default size (no larger than nine tenths of the monitor), centred on the primary monitor.
fn default_placement(monitors: &[Monitor], zoom: f32) -> Placement {
    let Some(monitor) = monitors.iter().find(|m| m.primary).or_else(|| monitors.first()) else {
        return Placement { position: None, size: DEFAULT_SIZE, maximized: false };
    };
    let per_point = monitor.scale * zoom;
    let size = DEFAULT_SIZE.min(monitor.rect.size() * 0.9 / per_point).max(MIN_SIZE);
    let corner = monitor.rect.center() - size * per_point / 2.0;
    Placement { position: Some(corner / per_point), size, maximized: false }
}

/// egui's zoom as saved, or 1 when it is out of egui's range.
fn sane_zoom(zoom: f32) -> f32 {
    if (0.2..=5.0).contains(&zoom) { zoom } else { 1.0 }
}

/// Finite, at least a pixel each way, and not parked where Windows puts a minimized window.
fn plausible(rect: Rect) -> bool {
    rect.is_finite() && rect.width() >= 1.0 && rect.height() >= 1.0 && !(rect.min.x <= MINIMIZED_CORNER && rect.min.y <= MINIMIZED_CORNER)
}

/// What to save after a frame in which egui saw the window as `info`, given what was to be saved
/// before (`last`). `pixels_per_point` is egui's (the monitor's scale times `zoom`).
pub fn observed(last: Option<Saved>, info: &egui::ViewportInfo, pixels_per_point: f32, zoom: f32) -> Option<Saved> {
    if info.minimized == Some(true) || info.fullscreen == Some(true) {
        return last;
    }
    if info.maximized == Some(true) {
        return last.map(|s| Saved { maximized: true, ..s });
    }
    let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) else { return last };
    normal_window(outer, inner, pixels_per_point, zoom).or(last)
}

/// A normal window's [`Saved`], from its frame and content in points, if they are plausible.
fn normal_window(outer: Rect, inner: Rect, pixels_per_point: f32, zoom: f32) -> Option<Saved> {
    let (outer, inner) = (outer.min * pixels_per_point, Rect::from_min_size(inner.min * pixels_per_point, inner.size() * pixels_per_point));
    (plausible(inner) && outer.is_finite()).then(|| Saved {
        version: VERSION,
        outer: [outer.x, outer.y],
        inner: [inner.min.x, inner.min.y, inner.width(), inner.height()],
        zoom,
        maximized: false,
    })
}

/// The window's file, beside the UI state's at `ui_state`: its name with `.window.json` for its
/// extension.
pub fn file_beside(ui_state: &Path) -> PathBuf {
    ui_state.with_extension("window.json")
}

/// The geometry saved at `path`, or `None` (said on stderr, unless there is no file).
pub fn load(path: &Path) -> Option<Saved> {
    match read(path) {
        Ok(saved) => saved,
        Err(why) => {
            eprintln!("{}: the window's size and place at {} weren't read, so it opens at the default: {why}", env!("CARGO_PKG_NAME"), path.display());
            None
        }
    }
}

fn read(path: &Path) -> Result<Option<Saved>, String> {
    let Some(text) = files::read_capped_text(path, MAX_FILE).map_err(|e| e.message)? else { return Ok(None) };
    let saved: Saved = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if saved.version != VERSION {
        return Err(format!("version {} is not {VERSION}", saved.version));
    }
    Ok(Some(saved))
}

/// Writes `saved` to `path`, atomically.
pub fn save(path: &Path, saved: &Saved) -> Result<(), String> {
    let text = serde_json::to_string_pretty(saved).map_err(|e| e.to_string())?;
    files::write_atomic(path, text.as_bytes()).map_err(|e| e.message)
}

/// Follows the window through a run, and writes what [`observed`] keeps when the window closes.
#[derive(Debug)]
pub struct Tracker {
    path: Option<PathBuf>,
    saved: Option<Saved>,
    written: Option<Saved>,
}

impl Tracker {
    /// A tracker writing to `path` (none: nothing is kept), starting from what the last run saved.
    pub fn new(path: Option<PathBuf>, loaded: Option<Saved>) -> Tracker {
        Tracker { path, saved: loaded, written: loaded }
    }

    /// Notes the window as egui sees it this frame.
    pub fn observe(&mut self, ctx: &egui::Context) {
        let zoom = ctx.zoom_factor();
        self.saved = ctx.input(|i| observed(self.saved, i.viewport(), i.pixels_per_point, zoom));
    }

    /// Writes the geometry, if it changed since the last write. A failure is said, never fatal.
    pub fn save(&mut self) {
        let (Some(path), Some(saved)) = (&self.path, self.saved) else { return };
        if self.written == Some(saved) {
            return;
        }
        match save(path, &saved) {
            Ok(()) => self.written = Some(saved),
            Err(why) => eprintln!("{}: the window's size and place weren't saved to {}: {why}", env!("CARGO_PKG_NAME"), path.display()),
        }
    }
}

/// The monitors, filled in when the event loop starts and read when eframe builds the window, behind
/// the app's own lock.
pub type Monitors = Arc<Lock<Vec<Monitor>>>;

/// An empty [`Monitors`].
pub fn monitors() -> Monitors {
    Arc::new(Lock::new("the monitors", Vec::new()))
}

/// The connected monitors (any with no area left out).
fn read_monitors(event_loop: &ActiveEventLoop) -> Vec<Monitor> {
    let primary = event_loop.primary_monitor();
    let monitor = |m: winit::monitor::MonitorHandle| {
        let (pos, size) = (m.position(), m.size());
        let rect = Rect::from_min_size(pos2(pos.x as f32, pos.y as f32), vec2(size.width as f32, size.height as f32));
        let scale = m.scale_factor() as f32;
        (rect.area() > 0.0 && scale.is_finite() && scale > 0.0).then(|| Monitor { rect, scale, primary: primary.as_ref() == Some(&m) })
    };
    event_loop.available_monitors().filter_map(monitor).collect()
}

/// eframe's window hook: the viewport eframe built, placed by [`placement`].
pub fn hook(saved: Option<Saved>, monitors: Monitors) -> eframe::WindowBuilderHook {
    Box::new(move |builder| {
        let monitors = monitors.with(|m| m.clone()).unwrap_or_default();
        placement(saved.as_ref(), &monitors).apply(builder)
    })
}

/// eframe's application on the app's own event loop, reading the monitors into [`Monitors`] at the
/// first `resumed`, just before eframe opens the window there.
pub struct Launch<'a> {
    app: eframe::EframeWinitApplication<'a>,
    monitors: Monitors,
}

impl<'a> Launch<'a> {
    /// Runs eframe's app, named `name`, until the window closes.
    pub fn run(name: &str, options: eframe::NativeOptions, monitors: Monitors, creator: eframe::AppCreator<'a>) -> Result<(), String> {
        let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
        let app = eframe::create_native(name, options, creator, &event_loop);
        event_loop.run_app(&mut Launch { app, monitors }).map_err(|e| e.to_string())
    }
}

impl ApplicationHandler<UserEvent> for Launch<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let _ = self.monitors.with(|monitors| {
            if monitors.is_empty() {
                *monitors = read_monitors(event_loop);
            }
        });
        self.app.resumed(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        self.app.window_event(event_loop, window_id, event);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.app.new_events(event_loop, cause);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.app.user_event(event_loop, event);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, device_id: DeviceId, event: DeviceEvent) {
        self.app.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.app.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.app.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.app.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.app.memory_warning(event_loop);
    }
}

#[cfg(test)]
mod tests;
