# GPUI, patched for Lumen

A copy of [gpui 0.2.2](https://crates.io/crates/gpui) (Apache-2.0, by Zed
Industries; see `LICENSE-APACHE`), used through `[patch.crates-io]` in
Lumen's `Cargo.toml`. The examples and tests are left out (and their entries
removed from `Cargo.toml`). `lumen.patch` is the diff of the source changes
against the published crate.

## Draw as soon as a window changes (X11)

On X11, GPUI draws only when its refresh timer ticks, once per display frame.
New terminal output waited for the next tick, which added up to a frame
(8 ms at 120 Hz) to every echoed keypress.

- `src/window.rs`: the window's invalidator calls a callback when the window
  goes from clean to dirty.
- `src/platform.rs`: `PlatformWindow::frame_requester` supplies that callback;
  by default there is none.
- `src/platform/linux/x11/{window,client}.rs`: each X11 window registers a
  calloop ping. The callback pings it, and the handler (`draw_now`) draws the
  window straight away, at most once per refresh interval. Anything dirtied
  again sooner is left to the timer, so a flood of output still draws at the
  display rate. Hidden windows wait for the timer as before.

Measured from writing to the pty until the pixels change on screen, at 120 Hz
under Cinnamon: median 7.4–11.4 ms before, 4.9–5.9 ms after. Throughput and
CPU use under heavy output were unchanged.

Wayland and macOS already draw on the compositor's frame callback or the
display link, so they are unchanged.

## Set up the GPU while the app starts (X11)

Creating the Vulkan device takes the NVIDIA driver about a quarter of a
second, and GPUI did it on the main thread before anything else could run.

- `src/platform/linux/x11/client.rs`: `GpuSetup` creates the `BladeContext`
  on its own thread as soon as the X11 client starts; the first
  `open_window` waits for it. If the thread can't be started, it is created
  in place as before, and a failure is reported by `open_window`.
- `src/platform/blade/blade_context.rs`: `BladeContext` is `Clone` on Linux
  too (it is an `Arc`), so the waiting window can take a copy.

Meanwhile GPUI loads the system fonts and Lumen starts the panes' shells, so
they're ready by the first frame. With bash at 120 Hz on an NVIDIA GPU, the
first prompt went from 516–600 ms after launch to 345–407 ms.

## Don't miss events read while waiting for a reply (X11)

When GPUI waits for the reply to an X request, x11rb reads any events that
arrived first into its own queue. The event loop wakes only when the socket
has unread data, so those queued events sat there until something else
arrived. Under Metacity this caught the new window's MapNotify at startup:
GPUI never learnt the window was shown, so it never drew it, and the window
stayed blank until it was resized.

- `src/platform/linux/x11/client.rs`: `run` drains x11rb's queue after every
  pass of the event loop, before it sleeps again.

## The window's X11 id (X11)

`src/platform/linux/x11/window.rs`: `X11Window` implements
`HasWindowHandle`, returning its X window, where upstream has
`unimplemented!()` (so asking for it panicked). Lumen's quick terminal uses
the id to set window-manager hints (above other windows, on every
workspace, out of the taskbar) and to hide and show the window.

## Build fix

`src/taffy.rs`: two float literals given an explicit `f32` type. Newer Rust
warns that the old form will become an error.

## Upgrading

When moving to a newer gpui, check whether it draws on demand on X11. If it
doesn't, reapply `lumen.patch` to a fresh copy; otherwise drop this copy and
the `[patch.crates-io]` entry.
