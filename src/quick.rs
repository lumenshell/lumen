//! The quick terminal: a Lumen window that drops down from the top of the
//! screen when you press a key from anywhere, and hides again when you press
//! it again, keeping its shells running.
//!
//! On X11 Lumen manages the window itself: it sits above other windows, on
//! every workspace, out of the taskbar and switcher, full width at the top of
//! the monitor the pointer is on, and hiding it unmaps it. The key comes from
//! your desktop's shortcut settings (`lumen msg quick-terminal`), or Lumen
//! grabs `quick-terminal-key` itself. Elsewhere (Wayland, macOS) it's a
//! normal window that toggles between shown and minimized.

use gpui::{App, AppContext, Bounds, Global, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, point, px, size};

use crate::settings::Settings;
use crate::workspace::Workspace;

/// The quick terminal's window, once it has been opened.
struct Quick {
    window: WindowHandle<Workspace>,
    /// Its X11 window, when Lumen manages it.
    xid: Option<u32>,
    shown: bool,
    /// It has been the active window since it was last shown; losing that
    /// (with `quick-terminal-autohide`) hides it.
    was_active: bool,
}

#[derive(Default)]
struct QuickState(Option<Quick>);

impl Global for QuickState {}

/// Show the quick terminal, or hide it if it's in front. Opens it the first
/// time. Returns whether it's shown now.
pub fn toggle(cx: &mut App) -> bool {
    let quick = cx.default_global::<QuickState>().0.take();
    let Some(mut quick) = quick.filter(|q| q.window.update(cx, |_, _, _| ()).is_ok()) else {
        open(cx);
        return true;
    };
    // Ask the window manager on X11: GPUI's own idea flickers while a grabbed
    // key is held (the focused window gets a FocusOut for the grab).
    let active = match quick.xid {
        Some(xid) => x11::active_window() == Some(xid),
        None => quick.window.update(cx, |_, window, _| window.is_window_active()).unwrap_or(false),
    };
    if quick.shown && active {
        hide(&mut quick, cx);
    } else {
        show(&mut quick, cx);
    }
    let shown = quick.shown;
    cx.global_mut::<QuickState>().0 = Some(quick);
    shown
}

/// Whether this window is the quick terminal.
pub fn is_quick(window: gpui::AnyWindowHandle, cx: &App) -> bool {
    cx.try_global::<QuickState>().and_then(|s| s.0.as_ref()).is_some_and(|q| q.window.window_id() == window.window_id())
}

/// The quick terminal's window became active or stopped being: with
/// `quick-terminal-autohide`, clicking away hides it.
pub fn activation_changed(active: bool, cx: &mut App) {
    let Some(quick) = cx.default_global::<QuickState>().0.as_mut() else { return };
    if active {
        quick.was_active = true;
        return;
    }
    if !(quick.shown && quick.was_active && Settings::get(cx).quick_terminal_autohide) {
        return;
    }
    // Losing focus for a moment (a grabbed key, a menu) isn't clicking
    // away: hide only if another window is still active shortly after.
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
        let _ = cx.update(|cx| {
            let Some(mut quick) = cx.default_global::<QuickState>().0.take() else { return };
            let still_away = match quick.xid {
                Some(xid) => x11::active_window() != Some(xid),
                None => !quick.window.update(cx, |_, window, _| window.is_window_active()).unwrap_or(true),
            };
            if quick.shown && still_away {
                hide(&mut quick, cx);
            }
            cx.global_mut::<QuickState>().0 = Some(quick);
        });
    })
    .detach();
}

fn open(cx: &mut App) {
    let height = Settings::get(cx).quick_terminal_height;
    let scale = cx
        .windows()
        .first()
        .and_then(|w| w.update(cx, |_, window, _| window.scale_factor()).ok())
        .unwrap_or(1.0);
    // Where it goes, in logical pixels: the top of the pointer's monitor.
    let area = monitor_under_pointer()
        .map(|(x, y, w, h)| {
            Bounds::new(point(px(x as f32 / scale), px(y as f32 / scale)), size(px(w as f32 / scale), px(h as f32 / scale)))
        })
        .or_else(|| cx.primary_display().map(|d| d.bounds()))
        .unwrap_or_else(|| Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(800.0))));
    let bounds = Bounds::new(area.origin, size(area.size.width, area.size.height * height));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: crate::workspace::window_background(Settings::get(cx).background_opacity),
        window_decorations: Some(WindowDecorations::Client),
        window_min_size: Some(size(px(320.0), px(160.0))),
        app_id: Some("lumen".into()),
        ..Default::default()
    };
    let Ok(window) = cx.open_window(options, |window, cx| cx.new(|cx| Workspace::new_quick(window, cx))) else {
        eprintln!("lumen: couldn't open the quick terminal");
        return;
    };
    let xid = window.update(cx, |_, window, _| x11::window_id(window)).ok().flatten();
    let mut quick = Quick { window, xid, shown: true, was_active: false };
    show(&mut quick, cx);
    cx.default_global::<QuickState>().0 = Some(quick);
}

fn show(quick: &mut Quick, cx: &mut App) {
    quick.shown = true;
    quick.was_active = false;
    if let Some(xid) = quick.xid {
        let height = Settings::get(cx).quick_terminal_height;
        x11::show(xid, height);
    }
    let _ = quick.window.update(cx, |ws, window, cx| {
        window.activate_window();
        ws.focus_active_pane(window, cx);
    });
}

fn hide(quick: &mut Quick, cx: &mut App) {
    quick.shown = false;
    match quick.xid {
        Some(xid) => x11::hide(xid),
        None => {
            let _ = quick.window.update(cx, |_, window, _| window.minimize_window());
        }
    }
}

/// The monitor under the pointer, in device pixels: x, y, width, height.
fn monitor_under_pointer() -> Option<(i32, i32, u32, u32)> {
    x11::monitor_under_pointer()
}

#[cfg(target_os = "linux")]
mod x11 {
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, InputFocus, PropMode, StackMode,
        UnmapNotifyEvent, UNMAP_NOTIFY_EVENT,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    /// Lumen talks to X11 itself only on X11, not through XWayland.
    fn connect() -> Option<(RustConnection, u32)> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return None;
        }
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen].root;
        Some((conn, root))
    }

    fn atom(conn: &RustConnection, name: &[u8]) -> Option<u32> {
        Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
    }

    pub fn window_id(window: &gpui::Window) -> Option<u32> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return None;
        }
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Xcb(h) => Some(h.window.get()),
            RawWindowHandle::Xlib(h) => Some(h.window as u32),
            _ => None,
        }
    }

    pub fn monitor_under_pointer() -> Option<(i32, i32, u32, u32)> {
        let (conn, root) = connect()?;
        let pointer = conn.query_pointer(root).ok()?.reply().ok()?;
        let (px, py) = (i32::from(pointer.root_x), i32::from(pointer.root_y));
        let monitors = conn.randr_get_monitors(root, true).ok()?.reply().ok()?.monitors;
        let inside = |m: &&x11rb::protocol::randr::MonitorInfo| {
            let (x, y) = (i32::from(m.x), i32::from(m.y));
            (x..x + i32::from(m.width)).contains(&px) && (y..y + i32::from(m.height)).contains(&py)
        };
        let m = monitors.iter().find(inside).or_else(|| monitors.iter().find(|m| m.primary)).or(monitors.first())?;
        Some((i32::from(m.x), i32::from(m.y), u32::from(m.width), u32::from(m.height)))
    }

    /// Put the window at the top of the pointer's monitor, above everything,
    /// on every workspace and out of the taskbar, and give it the keyboard.
    pub fn show(xid: u32, height: f32) {
        let Some((conn, root)) = connect() else { return };
        let names: [&[u8]; 4] =
            [b"_NET_WM_STATE_ABOVE", b"_NET_WM_STATE_STICKY", b"_NET_WM_STATE_SKIP_TASKBAR", b"_NET_WM_STATE_SKIP_PAGER"];
        let states: Vec<u32> = names.iter().filter_map(|n| atom(&conn, n)).collect();
        let (Some(wm_state), Some(active), Some(desktop)) =
            (atom(&conn, b"_NET_WM_STATE"), atom(&conn, b"_NET_ACTIVE_WINDOW"), atom(&conn, b"_NET_WM_DESKTOP"))
        else {
            return;
        };
        // While it's hidden (unmapped) the window manager reads these when it
        // maps; while it's shown, it takes requests for them.
        let _ = conn.change_property32(PropMode::REPLACE, xid, wm_state, AtomEnum::ATOM, &states);
        let _ = conn.change_property32(PropMode::REPLACE, xid, desktop, AtomEnum::CARDINAL, &[0xFFFF_FFFF]);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        for pair in states.chunks(2) {
            let data = [1, pair[0], pair.get(1).copied().unwrap_or(0), 2, 0];
            let ev = ClientMessageEvent::new(32, xid, wm_state, data);
            let _ = conn.send_event(false, root, mask, ev);
        }
        if let Some((x, y, w, h)) = monitor_under_pointer() {
            let h = ((h as f32 * height).round() as u32).max(160);
            let aux = ConfigureWindowAux::new().x(x).y(y).width(w).height(h).stack_mode(StackMode::ABOVE);
            let _ = conn.configure_window(xid, &aux);
        }
        let _ = conn.map_window(xid);
        // Source 2: a pager-style request, which focus-stealing prevention lets
        // through; you pressed a key for this.
        let ev = ClientMessageEvent::new(32, xid, active, [2, x11rb::CURRENT_TIME, 0, 0, 0]);
        let _ = conn.send_event(false, root, mask, ev);
        let _ = conn.set_input_focus(InputFocus::PARENT, xid, x11rb::CURRENT_TIME);
        let _ = conn.sync();
        // Window managers place a window again when they map it: put it back.
        if let Some((x, y, w, h)) = monitor_under_pointer() {
            let h = ((h as f32 * height).round() as u32).max(160);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(60));
                if let Some((conn, _)) = connect() {
                    let _ = conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y).width(w).height(h));
                    let _ = conn.sync();
                }
            });
        }
    }

    /// The window the window manager says is active.
    pub fn active_window() -> Option<u32> {
        let (conn, root) = connect()?;
        let active = atom(&conn, b"_NET_ACTIVE_WINDOW")?;
        let reply = conn.get_property(false, root, active, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?;
        reply.value32()?.next().filter(|&w| w != 0)
    }

    /// Withdraw the window (ICCCM: unmap it and tell the root window), which
    /// hides it everywhere while its shells keep running.
    pub fn hide(xid: u32) {
        let Some((conn, root)) = connect() else { return };
        let _ = conn.unmap_window(xid);
        let ev = UnmapNotifyEvent { response_type: UNMAP_NOTIFY_EVENT, sequence: 0, event: root, window: xid, from_configure: false };
        let _ = conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
        let _ = conn.sync();
    }
}

#[cfg(not(target_os = "linux"))]
mod x11 {
    pub fn window_id(_: &gpui::Window) -> Option<u32> {
        None
    }
    pub fn monitor_under_pointer() -> Option<(i32, i32, u32, u32)> {
        None
    }
    pub fn show(_: u32, _: f32) {}
    pub fn active_window() -> Option<u32> {
        None
    }
    pub fn hide(_: u32) {}
}

/// The key grabbed for the quick terminal, kept in step with
/// `quick-terminal-key`.
struct HotkeyState {
    key: String,
    _grab: Option<crate::hotkey::Grab>,
    /// Why the key couldn't be grabbed, shown in the banner.
    problem: Option<String>,
}

impl Global for HotkeyState {}

/// Grab `quick-terminal-key` now and again whenever the config changes.
pub fn watch_hotkey(cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    cx.spawn(async move |cx| {
        use futures::StreamExt;
        while rx.next().await.is_some() {
            if cx.update(toggle).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.set_global(HotkeyState { key: String::new(), _grab: None, problem: None });
    let regrab = move |cx: &mut App| {
        let key = Settings::get(cx).quick_terminal_key.clone();
        if cx.global::<HotkeyState>().key == key {
            // Settings reloaded (dropping our note from the banner): put it back.
            if let Some(p) = cx.global::<HotkeyState>().problem.clone()
                && !Settings::get(cx).problems.contains(&p)
            {
                cx.global_mut::<Settings>().problems.push(p);
            }
            return;
        }
        // Release the old key before trying the new one.
        cx.global_mut::<HotkeyState>()._grab = None;
        let tx = tx.clone();
        let result = match crate::hotkey::parse(&key) {
            Ok(Some(hotkey)) => crate::hotkey::grab(hotkey, move || {
                let _ = tx.unbounded_send(());
            })
            .map(Some),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        };
        let (grab, problem) = match result {
            Ok(grab) => (grab, None),
            Err(e) => (None, Some(format!("quick-terminal-key \"{key}\": {e}"))),
        };
        if let Some(p) = &problem {
            cx.global_mut::<Settings>().problems.push(p.clone());
        }
        cx.set_global(HotkeyState { key, _grab: grab, problem });
    };
    regrab(cx);
    cx.observe_global::<Settings>(regrab).detach();
}
