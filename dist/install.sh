#!/bin/sh
# Install Lumen, or update it to the latest release:
#
#   curl -fsSL https://lumenshell.dev/install | sh
#
# Options (after `sh -s --` when piping):
#   --uninstall      remove Lumen (your settings in ~/.config/lumen stay)
#
# Environment:
#   LUMEN_VERSION=0.2.0       install that version instead of the latest
#   LUMEN_BIN_DIR=~/bin       where the `lumen` command goes (default ~/.local/bin)
#   LUMEN_DOWNLOAD_URL=...    download from here instead of GitHub releases
#                             (a folder holding the release files, for mirrors and testing)
#
# Linux: the binary goes in ~/.local/bin, with a menu entry and icons in
# ~/.local/share. macOS: Lumen.app goes in /Applications (or ~/Applications
# when /Applications isn't writable), with `lumen` linked into ~/.local/bin.
# Nothing needs sudo.

# Everything is in functions and main runs on the last line, so a download cut
# off partway through runs nothing.
set -eu

REPO=lumenshell/lumen

say() { printf '%s\n' "$*"; }
step() { printf '==> %s\n' "$*"; }
fail() { printf 'lumen install: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }
# A path as people write it: ~/.local/bin, not /home/you/.local/bin.
tilde() { case "$1" in "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;; *) printf '%s' "$1" ;; esac; }

download() { # url file
    if has curl; then
        curl -fsSL --proto '=https,file' --retry 3 -o "$2" "$1"
    elif has wget; then
        wget -q -O "$2" "$1"
    else
        fail "this needs curl or wget to download Lumen"
    fi
}

sha256() {
    if has sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        fail "this needs sha256sum or shasum to check the download"
    fi
}

# Download one release file into $tmp and check it against SHA256SUMS.
fetch() { # name
    download "$base/$1" "$tmp/$1" || fail "couldn't download $base/$1"
    want=$(awk -v f="$1" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$want" ] || fail "SHA256SUMS doesn't list $1"
    got=$(sha256 "$tmp/$1")
    [ "$got" = "$want" ] || fail "$1 is corrupt (its checksum doesn't match); try again"
}

refresh_menu() {
    if has gtk-update-icon-cache; then
        gtk-update-icon-cache -f -t "$data/icons/hicolor" >/dev/null 2>&1 || true
    fi
    if has update-desktop-database; then
        update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    fi
}

install_linux() {
    case $(uname -m) in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) fail "there's no Lumen build for $(uname -m) Linux yet; build it from source: https://github.com/$REPO#build" ;;
    esac
    name=lumen-linux-$arch
    step "Downloading Lumen $label for Linux $arch"
    fetch "$name.tar.gz"
    tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
    pkg=$tmp/$name
    open_hint="Open Lumen from your app menu, or run: lumen"

    step "Installing $(tilde "$bindir")/lumen"
    mkdir -p "$bindir"
    # Copy beside the old one and rename over it: replacing a running binary
    # in place fails ("text file busy"), but renaming over it is fine.
    cp "$pkg/bin/lumen" "$bindir/.lumen.new"
    chmod 755 "$bindir/.lumen.new"
    mv -f "$bindir/.lumen.new" "$bindir/lumen"

    step "Adding Lumen to the app menu"
    mkdir -p "$data/applications"
    mkdir -p "$data/icons"
    cp -R "$pkg/share/icons/hicolor" "$data/icons/"
    # App launchers don't always have ~/.local/bin on their PATH, so the menu
    # entry runs Lumen by its full path, quoted in case it has spaces.
    case "$bindir" in
        *[\"\`\$\\]*) say "    (skipped: the menu entry can't hold the path $(tilde "$bindir"))" ;;
        *)
            while IFS= read -r line; do
                case "$line" in
                    Exec=*) printf 'Exec="%s"\n' "$bindir/lumen" ;;
                    *) printf '%s\n' "$line" ;;
                esac
            done < "$pkg/share/applications/lumen.desktop" > "$data/applications/lumen.desktop"
            chmod 644 "$data/applications/lumen.desktop"
            refresh_menu
            ;;
    esac

    # The release is built against a few common X11 libraries; say which are
    # missing rather than leaving Lumen to fail at start.
    if has ldd; then
        missing=$(ldd "$bindir/lumen" 2>/dev/null | awk '/not found/ { print $1 }' | tr '\n' ' ')
        if [ -n "$missing" ]; then
            say ""
            say "Lumen needs these libraries, which aren't installed: $missing"
            say "On Debian or Ubuntu:  sudo apt install libxkbcommon-x11-0 libxcb1 libvulkan1"
            say "On Fedora:            sudo dnf install libxkbcommon-x11 libxcb vulkan-loader"
            say "On Arch:              sudo pacman -S libxkbcommon-x11 libxcb vulkan-icd-loader"
        fi
    fi
}

install_macos() {
    step "Downloading Lumen $label for macOS"
    fetch lumen-macos.zip
    ditto -x -k "$tmp/lumen-macos.zip" "$tmp/app"
    [ -d "$tmp/app/Lumen.app" ] || fail "the download doesn't hold Lumen.app"

    apps=$(macos_apps_dir)
    open_hint="Open Lumen from $(tilde "$apps"), or run: lumen"
    step "Installing $(tilde "$apps")/Lumen.app"
    mkdir -p "$apps"
    rm -rf "$apps/.Lumen.app.new"
    mv "$tmp/app/Lumen.app" "$apps/.Lumen.app.new"
    rm -rf "$apps/Lumen.app"
    mv "$apps/.Lumen.app.new" "$apps/Lumen.app"
    # Files curl downloads aren't quarantined, but wget's or a mirror's might be.
    xattr -dr com.apple.quarantine "$apps/Lumen.app" 2>/dev/null || true

    step "Linking $(tilde "$bindir")/lumen"
    mkdir -p "$bindir"
    ln -sf "$apps/Lumen.app/Contents/MacOS/lumen" "$bindir/lumen"
}

# /Applications if Lumen is already there or it's writable; else ~/Applications.
macos_apps_dir() {
    if [ -d "$HOME/Applications/Lumen.app" ]; then
        echo "$HOME/Applications"
    elif [ -w /Applications ]; then
        echo /Applications
    else
        echo "$HOME/Applications"
    fi
}

uninstall() {
    rm -f "$bindir/lumen"
    case $(uname -s) in
        Linux)
            rm -f "$data/applications/lumen.desktop"
            for f in "$data"/icons/hicolor/*/apps/lumen.png "$data/icons/hicolor/scalable/apps/lumen.svg"; do
                [ -e "$f" ] && rm -f "$f"
            done
            refresh_menu
            ;;
        Darwin)
            for apps in /Applications "$HOME/Applications"; do
                if [ -d "$apps/Lumen.app" ]; then
                    rm -rf "$apps/Lumen.app" || say "Couldn't remove $apps/Lumen.app; drag it to the Bin."
                fi
            done
            ;;
    esac
    say "Removed Lumen. Your settings in $(tilde "${XDG_CONFIG_HOME:-$HOME/.config}")/lumen are still there."
}

main() {
    bindir=${LUMEN_BIN_DIR:-$HOME/.local/bin}
    data=${XDG_DATA_HOME:-$HOME/.local/share}

    case "${1:-}" in
        --uninstall) uninstall; return ;;
        -h | --help) sed -n '2,19s/^# \{0,1\}//p' "$0" 2>/dev/null || say "See https://lumenshell.dev"; return ;;
        "") ;;
        *) fail "unknown option $1 (try --help)" ;;
    esac

    version=${LUMEN_VERSION:-}
    version=${version#v}
    if [ -n "${LUMEN_DOWNLOAD_URL:-}" ]; then
        base=${LUMEN_DOWNLOAD_URL%/}
    elif [ -n "$version" ]; then
        base=https://github.com/$REPO/releases/download/v$version
    else
        base=https://github.com/$REPO/releases/latest/download
    fi
    label=${version:-(latest)}

    tmp=$(mktemp -d 2>/dev/null || mktemp -d -t lumen)
    trap 'rm -rf "$tmp"' EXIT
    download "$base/SHA256SUMS" "$tmp/SHA256SUMS" ||
        fail "couldn't download Lumen ${version:+$version }from $base"

    case $(uname -s) in
        Linux) install_linux ;;
        Darwin) install_macos ;;
        *) fail "Lumen runs on Linux and macOS, not $(uname -s)" ;;
    esac

    installed=$("$bindir/lumen" --version 2>/dev/null || echo "lumen")
    say ""
    say "Installed $installed."
    case ":$PATH:" in
        *":$bindir:"*) say "$open_hint" ;;
        *)
            case "$bindir" in
                "$HOME"/*) shown="\$HOME/${bindir#"$HOME"/}" ;;
                *) shown=$bindir ;;
            esac
            say "$(tilde "$bindir") isn't on your PATH, so the \`lumen\` command won't be found yet."
            say "Add this to your shell's startup file (~/.bashrc, ~/.zshrc):"
            say "    export PATH=\"$shown:\$PATH\""
            ;;
    esac
    if has pgrep && pgrep -x lumen >/dev/null 2>&1; then
        say "Lumen is running: quit and reopen it to start the new version."
    fi
}

main "$@"
