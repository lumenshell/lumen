# Installing Lumen

## The installer

```sh
curl -fsSL https://lumenshell.dev/install | sh
```

This installs the latest release for your user, without sudo.

- **Linux** (x86_64 and arm64): the `lumen` command goes in `~/.local/bin`,
  with a menu entry and icon.
- **macOS**: `Lumen.app` goes in Applications, and `lumen` is linked into
  `~/.local/bin`.

Run it again to update.

Two settings change what it does:

- `LUMEN_VERSION=0.2.0` installs a particular version.
- `LUMEN_BIN_DIR` puts the command somewhere else.

To remove Lumen (your settings stay):

```sh
curl -fsSL https://lumenshell.dev/install | sh -s -- --uninstall
```

## Homebrew (macOS)

```sh
brew install --cask lumenshell/tap/lumen
```

## The first launch on macOS

Apple hasn't notarized the app yet. The first time you open it, macOS may say
it can't check it for malware. Right-click Lumen.app, choose **Open**, then
**Open** again. Or allow it in System Settings > Privacy & Security.

## Installing by hand

Every release on the [releases page](https://github.com/lumenshell/lumen/releases)
has these files:

| File | What it is |
|---|---|
| `lumen-linux-<arch>.tar.gz` | Laid out like a prefix: `bin/`, `share/` |
| `lumen-macos.zip` | A universal Lumen.app |
| `SHA256SUMS` | Checksums for both |

## Building from source

On Debian or Ubuntu, install the build dependencies first.
`libnotify-bin` is for notifications.

```sh
sudo apt install libxkbcommon-x11-dev libxcb1-dev libfreetype-dev libfontconfig-dev \
                 libwayland-dev libvulkan1 libnotify-bin
```

Then run it, or install it:

```sh
cargo run --release

# installing also puts `lumen msg` on your PATH (~/.cargo/bin)
cargo install --locked --path .
```

`lumen` opens a window. `lumen msg ...` controls a running one (see
[Scripting](scripting.md)). `lumen --help` lists both.

### Trying the latest changes

`./install.sh` does it all. It builds this checkout, installs it, adds the app
menu entry and icon on Linux (or `~/Applications/Lumen.app` on macOS), and
says which commit you're on.

`./install.sh --restart` also closes the running Lumen and starts the new one.
This closes the shells open in it.

### Linux app menu and icon

After `cargo install`, run `scripts/install-linux.sh` to add Lumen to your app
menu with its icon, for your user only. The taskbar and Alt+Tab switcher then
show the icon too, because they match Lumen's windows to the menu entry.
Restart Lumen if it was open.

`scripts/install-linux.sh --uninstall` takes it out again.

**Packagers:** the entry is `assets/linux/lumen.desktop`, and the icons are a
`hicolor` theme tree in `assets/icons/linux/`.

### macOS app bundle

You only need the Xcode command-line tools. `scripts/bundle-macos.sh` builds
`target/release/Lumen.app` with Lumen's icon for the Dock and app switcher.
Drag it into Applications.

Its binary is `Lumen.app/Contents/MacOS/lumen`. Link it onto your PATH to use
`lumen msg`.
