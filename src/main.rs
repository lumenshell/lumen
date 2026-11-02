//! Lumen: a GPU-rendered terminal. Declares the actions, opens the window
//! (restoring its saved placement) and wires up settings and key bindings.

mod api;
mod boxdraw;
mod cli;
mod glyphs;
mod input;
mod keymap;
mod links;
mod marks;
mod mouse;
mod palette;
mod pty;
mod search;
mod session;
mod settings;
mod shell_integration;
mod terminal_view;
mod theme;
mod workspace;

use std::borrow::Cow;
use std::time::Duration;

use gpui::{
    Action, App, AppContext, Application, Bounds, Pixels, TitlebarOptions,
    WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowOptions, actions, point, px, size,
};

use settings::Settings;
use workspace::Workspace;

actions!(
    lumen,
    [
        NewTab,
        ClosePane,
        SplitRight,
        SplitDown,
        NextTab,
        PrevTab,
        NextPane,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        ToggleZoom,
        TogglePalette,
        NextTheme,
        IncreaseFont,
        DecreaseFont,
        ResetFont,
        Copy,
        Paste,
        Find,
        PrevPrompt,
        NextPrompt,
        CopyLastOutput,
        Quit,
    ]
);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = lumen, no_json)]
pub struct ActivateTab(pub usize);

fn load_fonts(cx: &App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
    ];
    cx.text_system().add_fonts(fonts).expect("failed to load bundled fonts");
}


fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("msg") {
        std::process::exit(cli::run(&args[1..]));
    }
    if let Some(arg) = args.first() {
        let help = matches!(arg.as_str(), "-h" | "--help" | "help");
        eprintln!("Usage: lumen            open a window\n       lumen msg ...      control a running Lumen (lumen msg help)");
        std::process::exit(if help { 0 } else { 2 });
    }
    Application::new().run(|cx: &mut App| {
        load_fonts(cx);
        cx.set_global(Settings::load());
        Settings::watch(cx);
        keymap::apply(cx);
        let listener = api::listen(cx);

        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &NextTheme, cx| {
            Settings::update(cx, |s| s.theme_index = (s.theme_index + 1) % s.themes.len())
        });
        cx.on_action(|_: &IncreaseFont, cx| Settings::update(cx, |s| s.font_size += 1.0));
        cx.on_action(|_: &DecreaseFont, cx| Settings::update(cx, |s| s.font_size -= 1.0));
        cx.on_action(|_: &ResetFont, cx| {
            Settings::update(cx, |s| s.font_size = settings::DEFAULT_FONT_SIZE)
        });

        let saved = session::load();
        let place = initial_placement(saved.as_ref().and_then(|s| s.window), cx);
        // X11 window managers ignore a maximize request sent before the window is
        // mapped, so on Linux open it normal and maximize once it's up (below).
        let window_bounds = if cfg!(target_os = "linux") {
            WindowBounds::Windowed(place.bounds)
        } else if place.fullscreen {
            WindowBounds::Fullscreen(place.bounds)
        } else if place.maximized {
            WindowBounds::Maximized(place.bounds)
        } else {
            WindowBounds::Windowed(place.bounds)
        };
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(window_bounds),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Lumen".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.0), px(15.0))),
                    }),
                    window_background: if cfg!(target_os = "macos") {
                        WindowBackgroundAppearance::Opaque
                    } else {
                        WindowBackgroundAppearance::Transparent
                    },
                    window_decorations: Some(WindowDecorations::Client),
                    window_min_size: Some(size(px(420.0), px(260.0))),
                    app_id: Some("lumen".into()),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(saved, window, cx)),
            )
            .expect("failed to open window");
        if let Some(listener) = listener {
            api::serve(listener, window, cx);
        }
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        #[cfg(target_os = "linux")]
        if place.restore_position || place.maximized || place.fullscreen {
            let scale = window.update(cx, |_, window, _| window.scale_factor()).unwrap_or(1.0);
            cx.spawn(async move |cx| {
                // The WM places new windows itself; move ours back once it's managed
                // (retrying briefly), so it also maximizes on the right monitor.
                for _ in 0..20 {
                    cx.background_executor().timer(Duration::from_millis(50)).await;
                    if !place.restore_position || x11_move(place.bounds, scale) {
                        break;
                    }
                }
                window
                    .update(cx, |_, window, _| {
                        if place.fullscreen {
                            window.toggle_fullscreen();
                        } else if place.maximized {
                            window.zoom_window();
                        }
                    })
                    .ok();
            })
            .detach();
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Placement {
    bounds: Bounds<Pixels>,
    /// The bounds came from the saved session (not a centred default).
    restore_position: bool,
    maximized: bool,
    fullscreen: bool,
}

/// Where to open the window: the saved placement if its title bar is still on a
/// connected display (monitors come and go), otherwise centred at the saved or
/// default size.
fn initial_placement(saved: Option<session::WindowState>, cx: &App) -> Placement {
    let Some(w) = saved else {
        return Placement {
            bounds: Bounds::centered(None, size(px(1100.0), px(720.0)), cx),
            restore_position: false,
            maximized: false,
            fullscreen: false,
        };
    };
    let min = size(px(420.0), px(260.0));
    let wanted = Bounds::new(point(px(w.x), px(w.y)), size(px(w.width), px(w.height)).max(&min));
    let titlebar = Bounds::new(wanted.origin, size(wanted.size.width, px(44.0)));
    let reachable = cx.displays().iter().any(|d| {
        let overlap = d.bounds().intersect(&titlebar);
        overlap.size.width >= px(120.0) && overlap.size.height >= px(20.0)
    });
    Placement {
        bounds: if reachable { wanted } else { Bounds::centered(None, wanted.size, cx) },
        restore_position: reachable,
        maximized: w.maximized,
        fullscreen: w.fullscreen,
    }
}

/// Move our X11 window so its visible frame (inside the shadow margin) has its
/// top-left at `bounds.origin`, unless that would leave the title bar off every
/// monitor; then the window manager's placement stands. Returns false if the WM
/// hasn't listed the window yet. GPUI doesn't expose the X window id, so find it
/// by our PID.
#[cfg(target_os = "linux")]
fn x11_move(bounds: Bounds<Pixels>, scale: f32) -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt};

    // Wayland clients can't position themselves; don't go looking via XWayland.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return true;
    }
    let Ok((conn, screen)) = x11rb::connect(None) else { return true };
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| Some(conn.intern_atom(true, name).ok()?.reply().ok()?.atom);
    let (Some(client_list), Some(wm_pid)) = (atom(b"_NET_CLIENT_LIST"), atom(b"_NET_WM_PID"))
    else {
        return true;
    };
    let property = |window, prop, kind: AtomEnum| {
        conn.get_property(false, window, prop, kind, 0, 4096).ok()?.reply().ok()
    };
    let Some(clients) = property(root, client_list, AtomEnum::WINDOW) else { return true };
    let me = std::process::id();
    let Some(ours) = clients.value32().into_iter().flatten().find(|&w| {
        property(w, wm_pid, AtomEnum::CARDINAL)
            .and_then(|p| p.value32()?.next())
            .is_some_and(|pid| pid == me)
    }) else {
        return false;
    };
    // GPUI reports one display per X screen, which spans every monitor, so ask
    // RandR for the real monitor rectangles (device pixels).
    let device = |v: f32| (v * scale).round() as i32;
    let (x, y) = (device(f32::from(bounds.origin.x)), device(f32::from(bounds.origin.y)));
    let (width, bar) = (device(f32::from(bounds.size.width)), device(44.0));
    if let Some(monitors) = conn.randr_get_monitors(root, true).ok().and_then(|c| c.reply().ok()) {
        let reachable = monitors.monitors.iter().any(|m| {
            let (mx, my) = (i32::from(m.x), i32::from(m.y));
            let overlap_w = (x + width).min(mx + i32::from(m.width)) - x.max(mx);
            let overlap_h = (y + bar).min(my + i32::from(m.height)) - y.max(my);
            overlap_w >= device(120.0) && overlap_h >= device(20.0)
        });
        if !reachable {
            return true;
        }
    }
    let inset = device(workspace::SHADOW_INSET);
    let aux = ConfigureWindowAux::new().x(x - inset).y(y - inset);
    // Wait for the server to process the request before the connection drops;
    // a fire-and-forget request was sometimes lost.
    if let Ok(cookie) = conn.configure_window(ours, &aux) {
        let _ = cookie.check();
    }
    true
}
