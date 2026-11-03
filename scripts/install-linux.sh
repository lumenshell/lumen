#!/bin/sh
# Put Lumen in your app menu with its icon, for your user only. The taskbar
# and Alt+Tab switcher then show the icon too: they match Lumen's windows
# (class/app id "lumen") to this menu entry.
#
#   scripts/install-linux.sh              install (after `cargo install --locked --path .`)
#   scripts/install-linux.sh --uninstall  remove what it installed
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
icons=$data/icons/hicolor
apps=$data/applications

refresh() {
    # Not every desktop needs these; skip them quietly where they're missing.
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$icons" >/dev/null 2>&1 || true
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$apps" >/dev/null 2>&1 || true
    fi
}

case "${1:-}" in
    --uninstall)
        rm -f "$apps/lumen.desktop"
        for f in "$icons"/*/apps/lumen.png "$icons"/scalable/apps/lumen.svg; do
            [ -e "$f" ] && rm -f "$f"
        done
        refresh
        echo "Removed Lumen's menu entry and icons."
        exit 0
        ;;
    "") ;;
    *)
        echo "Usage: $0 [--uninstall]" >&2
        exit 2
        ;;
esac

# The menu entry runs Lumen by its full path: app launchers don't always
# have ~/.cargo/bin on their PATH.
bin=$(command -v lumen 2>/dev/null || true)
if [ -z "$bin" ] && [ -x "$HOME/.cargo/bin/lumen" ]; then
    bin=$HOME/.cargo/bin/lumen
fi
if [ -z "$bin" ]; then
    echo "Lumen isn't installed yet. Run this first:  cargo install --locked --path ." >&2
    exit 1
fi
case "$bin" in
    *[\"\`\$\\]*) echo "Lumen's path ($bin) has characters a menu entry can't hold." >&2; exit 1 ;;
esac

for dir in "$repo"/assets/icons/linux/hicolor/*/apps; do
    size=$(basename "$(dirname "$dir")")
    mkdir -p "$icons/$size/apps"
    cp "$dir"/lumen.* "$icons/$size/apps/"
done

mkdir -p "$apps"
# Quote the path in case it has spaces (the desktop entry format allows that).
while IFS= read -r line; do
    case "$line" in
        Exec=*) printf 'Exec="%s"\n' "$bin" ;;
        *) printf '%s\n' "$line" ;;
    esac
done < "$repo/assets/linux/lumen.desktop" > "$apps/lumen.desktop"
chmod 644 "$apps/lumen.desktop"

refresh
echo "Installed Lumen's menu entry ($apps/lumen.desktop) and icons ($icons)."
echo "Restart Lumen to see its icon in the taskbar and switcher."
