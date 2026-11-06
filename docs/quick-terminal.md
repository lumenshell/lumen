# Quick terminal

The quick terminal is a Lumen window that drops down from the top of the
screen when you press a key, from any app. Press the key again and it goes
away.

Hiding it only hides it. What's running in it keeps going, so it comes back
exactly as you left it. It has tabs, splits and everything else a Lumen window
has.

## Giving it a key

There are two ways:

- **In your desktop's keyboard settings.** This works everywhere, including
  Wayland. Add a custom shortcut that runs `lumen msg quick-terminal`. In
  Cinnamon that's System Settings > Keyboard > Shortcuts > Custom Shortcuts.
  Lumen has to be running, so add it to your startup applications.
- **In `config.toml`** (X11 only). Set `quick-terminal-key = "f12"`, or any key
  such as `"ctrl-grave"` or `"super-space"`. Lumen catches it from every app
  while it's running. If another app already uses the key, the banner says so.

**Toggle Quick Terminal** in the palette does the same, as does the
`quick-terminal` action in `[keys]`.

## On X11

It opens full width at the top of the monitor the pointer is on. It sits
above other windows, on every workspace, and stays out of the taskbar and the
Alt+Tab switcher. Its height is `quick-terminal-height` of the screen (0.45 by
default).

- If it's shown but behind another window, the key brings it to the front.
- If it's in front, the key hides it.
- With `quick-terminal-autohide = true`, it also hides when you click into
  another window.

Closing its last pane closes it, and the next press opens a new one. It isn't
saved in the session. While it's open, Lumen keeps running even with no other
windows. Quit stops it.

## On Wayland and macOS

Apps can't place themselves at the top of the screen there. So the quick
terminal is an ordinary window that the key brings forward and minimizes.
