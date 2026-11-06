# Working on Lumen

Lumen is written in Rust. It draws with [GPUI](https://gpui.rs) (Zed's GPU UI
framework) and runs terminals with
[`alacritty_terminal`](https://crates.io/crates/alacritty_terminal).

- [Building](#building)
- [Tests](#tests)
- [Source layout](#source-layout)
- [Releasing](#releasing)

## Building

See [Building from source](install.md#building-from-source) for the
dependencies. Then:

```sh
cargo run --release
./install.sh             # build and install this checkout, with menu entry and icon
./install.sh --restart   # the same, then restart the running Lumen (closes its shells)
```

## Tests

```sh
cargo test
python3 -m unittest discover -s python/tests   # the Python package
python3 -m unittest discover -s plugins/tests  # the built-in plugins
```

## Source layout

| File                   | What it does                                              |
|------------------------|-----------------------------------------------------------|
| `src/main.rs`          | Actions, fonts, window creation                           |
| `src/keymap.rs`        | Default shortcuts and the `[keys]` table                  |
| `src/api.rs`           | Socket API server: JSON-RPC requests and the event stream |
| `src/cli.rs`           | The `lumen msg` command                                   |
| `src/info.rs`          | The `lumen info` command                                  |
| `src/campfire.rs`      | The `lumen campfire` command                              |
| `src/plugins.rs`       | Plugin manifests, approvals, running and restarting them  |
| `src/workspace.rs`     | Title bar, tabs, pane tree, focus halo, palette overlay   |
| `src/terminal_view.rs` | Grid rendering, cursor, selection, mouse and key input    |
| `src/pty.rs`           | Shell process + `alacritty_terminal` state and event loop; plugin panes, which a plugin draws instead |
| `src/input.rs`         | Keystroke → escape-sequence encoding, legacy and kitty keyboard protocol |
| `src/links.rs`         | URL and OSC 8 hyperlink detection                         |
| `src/search.rs`        | Find in scrollback: matching, navigation, highlights      |
| `src/scrollbar.rs`     | Scrollbar geometry: the thumb, command marks, and where a click or drag scrolls to |
| `src/vi.rs`            | Keyboard select mode: keys, motions, selections           |
| `src/glyphs.rs`        | Per-character glyph cache used to draw the grid           |
| `src/boxdraw.rs`       | Box-drawing, block and powerline characters as shapes     |
| `src/mouse.rs`         | Mouse event encoding for programs in mouse mode           |
| `src/palette.rs`       | Command palette                                           |
| `src/quick.rs`         | The quick terminal: opening, showing and hiding it, and its window hints (X11) |
| `src/hotkey.rs`        | `quick-terminal-key`: parsing it and grabbing it from every app (X11) |
| `src/theme.rs`         | Theme palettes and 256-colour table                       |
| `src/settings.rs`      | Settings global and config file                           |
| `src/session.rs`       | Saved windows and pane layouts (JSON) and loading/sanitising them |
| `src/shell_integration.rs` | Loading the shell scripts; turning their OSC 133/7 marks into hidden links |
| `src/marks.rs`         | Finding prompts, exit statuses and command output in the grid |
| `shell-integration/`   | The bash, zsh and fish scripts                            |
| `assets/icons/`        | The app icon: Linux `hicolor` sizes, macOS `Lumen.icns`, SVG sources |
| `assets/linux/`        | The `.desktop` menu entry                                 |
| `assets/screenshot.png` | The screenshot at the top of the README                 |
| `assets/screenshots/`  | The rest of the README's screenshots                    |
| `install.sh`           | Build and install this checkout for testing (menu entry, icon, `Lumen.app`) |
| `scripts/`             | `install-linux.sh` (menu entry and icons), `bundle-macos.sh` (`Lumen.app`), `package-linux.sh` (the Linux release tarball) |
| `dist/install.sh`      | The installer `lumenshell.dev/install` serves: downloads a release, checks it and installs it |
| `dist/homebrew/lumen.rb` | The Homebrew cask; each release fills it in and pushes it to `lumenshell/homebrew-tap` |
| `.github/workflows/release.yml` | Builds Linux and macOS releases when a `v*` tag is pushed ([how to release](#releasing)) |
| `examples/`            | Scripts using `lumen msg` and the socket API, and example plugins |
| `python/`              | `lumen_plugin`, the Python package for plugins (built into Lumen), and its tests |
| `plugins/`             | The built-in plugins (built into Lumen; turned on in the Plugin Gallery), and tests |
| `docs/`                | User guides and reference, linked from the README         |
| `vendor/xim-ctext/`    | A patched copy of the decoder GPUI's X11 input-method client uses, so input methods can't crash Lumen ([why](../vendor/xim-ctext/README-lumen.md)) |
| `vendor/gpui/`         | A patched copy of GPUI that draws on X11 as soon as output arrives instead of waiting for the next frame, sets up the GPU in the background, and gives out a window's X11 id ([why](../vendor/gpui/README-lumen.md)) |

## Releasing

1. Set `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` follows,
   and commit.
2. Tag it and push the tag: `git tag v0.2.0 && git push origin v0.2.0`.

The [release workflow](../.github/workflows/release.yml) then:

- builds Linux x86_64 and arm64 and a universal macOS app
- publishes the release with `SHA256SUMS`
- updates the cask in `lumenshell/homebrew-tap`

The Homebrew step needs a `HOMEBREW_TAP_TOKEN` secret: a fine-grained token
with Contents read and write on that repo.

A tag with a hyphen, like `v0.2.0-rc.1`, makes a pre-release and leaves
Homebrew alone.

To try a build without releasing, run the workflow by hand from the Actions
tab. The files are attached to the run.
