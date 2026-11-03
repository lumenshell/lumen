#!/usr/bin/env bash
# Build this checkout of Lumen and install it for your user, to test the
# latest changes for real.
#
#   ./install.sh            build, install, and set up the app menu/icon (Linux) or Lumen.app (macOS)
#   ./install.sh --restart  the same, then close any running Lumen and start the new one
#                           (this closes the shells open in it)
set -euo pipefail

cd "$(dirname "$0")"

usage() {
    sed -n '2,8s/^# \{0,1\}//p' "$0"
}

restart=0
for arg in "$@"; do
    case "$arg" in
        --restart) restart=1 ;;
        -h | --help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo isn't installed: get Rust from https://rustup.rs first." >&2
    exit 1
fi

# Which build this is, so you know what you're testing.
build="unknown"
if command -v git >/dev/null 2>&1 && git rev-parse --git-dir >/dev/null 2>&1; then
    build="$(git rev-parse --short HEAD)"
    if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
        build="$build plus uncommitted changes"
    fi
fi

echo "==> Building Lumen ($build)"
# --locked: build with the dependency versions in Cargo.lock, which are tested.
cargo install --locked --path .

case "$(uname -s)" in
    Linux)
        echo "==> Adding Lumen to the app menu"
        if [ "$restart" = 1 ]; then
            LUMEN_INSTALL_RESTARTS=1 scripts/install-linux.sh
        else
            scripts/install-linux.sh
        fi
        ;;
    Darwin)
        echo "==> Making Lumen.app"
        # cargo install built target/release/lumen; bundle that.
        scripts/bundle-macos.sh --no-build
        mkdir -p "$HOME/Applications"
        rm -rf "$HOME/Applications/Lumen.app"
        cp -R target/release/Lumen.app "$HOME/Applications/"
        echo "Installed $HOME/Applications/Lumen.app"
        ;;
esac

running="$(pgrep -x lumen -u "$(id -u)" || true)"
if [ "$restart" = 1 ]; then
    if [ -n "$running" ]; then
        echo "==> Closing the running Lumen"
        # shellcheck disable=SC2086 # one pid per word
        kill -TERM $running
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            pgrep -x lumen -u "$(id -u)" >/dev/null || break
            sleep 0.5
        done
        if pgrep -x lumen -u "$(id -u)" >/dev/null; then
            echo "Lumen didn't close; close it yourself and start it again." >&2
            exit 1
        fi
    fi
    echo "==> Starting the new Lumen"
    if [ "$(uname -s)" = Darwin ]; then
        open "$HOME/Applications/Lumen.app"
    else
        # Detached from this shell, so closing the terminal you ran this in
        # doesn't take Lumen with it.
        (cd "$HOME" && setsid "${CARGO_HOME:-$HOME/.cargo}/bin/lumen" >/dev/null 2>&1 < /dev/null &)
    fi
elif [ -n "$running" ]; then
    echo
    echo "Lumen is running the previous build. Restart it to test this one,"
    echo "or run ./install.sh --restart next time."
fi

echo
echo "Done: Lumen $build is installed."
