#!/usr/bin/env python3
"""Take Lumen's product screenshots, the same way every time.

Lumen runs on a virtual X display (Xvfb) at twice the usual pixel density,
with a demo home folder: a clean prompt, a small web shop project with a git
history, and stand-ins for `npm` so test runs and the dev server print the
same thing on every run. Each scene starts a fresh Lumen, lays out its tabs
and panes with `lumen msg`, presses keys where a scene needs the palette or
a plugin, and saves the window as a PNG.

    scripts/screenshots.py                 every scene
    scripts/screenshots.py palette find    just these
    scripts/screenshots.py --list          what there is

The main shot goes to assets/screenshot.png and the rest to
assets/screenshots/. Nothing outside a temporary folder is touched: your own
config, session and running Lumen windows are left alone.

Needs Linux with Xvfb, ImageMagick (`import`), python3-xlib, python3-pil and
Mesa's lavapipe Vulkan driver (mesa-vulkan-drivers), and a release build:
`cargo build --release`.
"""

import argparse
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter
from Xlib import X, XK, display as xdisplay
from Xlib.ext import xtest

ROOT = Path(__file__).resolve().parent.parent
LUMEN = ROOT / "target" / "release" / "lumen"
OUT = ROOT / "assets" / "screenshots"
LAVAPIPE = "/usr/share/vulkan/icd.d/lvp_icd.json"
SCALE = 2

# Window sizes in logical pixels. The main shot shows a whole workspace; the
# feature shots are close-ups: a small window, so the feature fills it and
# its text stays big when the image is shown small.
MAIN = (1280, 800)
FEATURE = (760, 475)


# ---- the demo home folder ----------------------------------------------------

BASHRC = r"""
export PATH="$HOME/bin:$PATH"
export HISTFILE=/dev/null
export GIT_PAGER=cat PAGER=cat
# The theme's own cyan and magenta, so the prompt suits light themes too.
PS1='\[\e[36m\]\w\[\e[0m\] \[\e[35m\]❯\[\e[0m\] '
alias ls='ls --color=auto'
"""

# A stand-in for npm: the same output on every run. `npm test` fails one test
# (so the prompt beside it gets Lumen's red mark), `npm test -- cart` passes,
# and `npm run dev` prints a dev server's banner and keeps running.
NPM = r"""#!/bin/sh
dim='\033[2m'; red='\033[31m'; green='\033[32m'; bold='\033[1m'; cyan='\033[36m'; off='\033[0m'
case "$*" in
  "run dev")
    printf "\n  ${green}${bold}VITE${off} ${green}v5.4.10${off}  ${dim}ready in${off} ${bold}312${off} ${dim}ms${off}\n\n"
    printf "  ${green}➜${off}  ${bold}Local${off}:   ${cyan}http://localhost:${bold}5173${off}${cyan}/${off}\n"
    printf "  ${dim}➜  Network: use --host to expose${off}\n\n"
    printf "${dim}10:41:07${off} ${cyan}[vite]${off} hmr update ${dim}/src/cart/CartDrawer.tsx${off}\n"
    printf "${dim}10:41:22${off} ${cyan}[vite]${off} hmr update ${dim}/src/checkout/Payment.tsx${off}\n"
    exec sleep 100000 ;;
  "test")
    printf "\n ${bold}RUN${off}  ${cyan}v2.1.4${off} ${dim}~/shop${off}\n\n"
    printf " ${green}✓${off} src/cart/cart.test.ts ${dim}(14 tests)${off} ${dim}38ms${off}\n"
    printf " ${green}✓${off} src/catalog/search.test.ts ${dim}(9 tests)${off} ${dim}21ms${off}\n"
    printf " ${red}✗${off} src/checkout/totals.test.ts ${dim}(6 tests | ${off}${red}1 failed${off}${dim})${off}\n"
    printf "   ${red}✗${off} applies the discount before tax\n"
    printf "     ${red}AssertionError: expected 107.99 to be 103.49${off}\n\n"
    printf " ${dim}Tests${off}  ${red}${bold}1 failed${off} ${dim}|${off} ${green}${bold}28 passed${off} ${dim}(29)${off}\n"
    exit 1 ;;
  *)
    printf "\n ${bold}RUN${off}  ${cyan}v2.1.4${off} ${dim}~/shop${off}\n\n"
    printf " ${green}✓${off} src/checkout/totals.test.ts ${dim}(6 tests)${off} ${dim}17ms${off}\n\n"
    printf " ${dim}Tests${off}  ${green}${bold}6 passed${off} ${dim}(6)${off}\n"
    exit 0 ;;
esac
"""

COMMITS = [
    ("Add the cart drawer", "Grace", "src/cart/CartDrawer.tsx"),
    ("Search the catalog as you type", "Ada", "src/catalog/search.ts"),
    ("Show stock levels on product cards", "Linus", "src/catalog/ProductCard.tsx"),
    ("Keep the cart across reloads", "Grace", "src/cart/store.ts"),
    ("Add Apple Pay to checkout", "Ada", "src/checkout/Payment.tsx"),
    ("Round totals half to even", "Margaret", "src/checkout/totals.ts"),
    ("Fix the coupon field losing focus", "Linus", "src/checkout/Coupon.tsx"),
    ("Lazy-load product images", "Grace", "src/catalog/Image.tsx"),
    ("Add an order history page", "Margaret", "src/account/Orders.tsx"),
    ("Apply discounts before tax", "Ada", "src/checkout/totals.ts"),
    ("Translate checkout into German", "Linus", "src/i18n/de.json"),
    ("Speed up the product grid", "Grace", "src/catalog/Grid.tsx"),
]

TODOS = [
    {"text": "Ship the cart drawer", "done": True},
    {"text": "Fix the rounding test", "done": False},
    {"text": "Review the Apple Pay PR", "done": False},
    {"text": "Write the 2.4 notes", "done": False},
]


def make_demo(base: Path) -> dict:
    """Build the demo home and Lumen's folders under `base`; return the env."""
    home, config, state, data = base / "home", base / "config", base / "state", base / "data"
    for d in (home / "bin", config / "lumen" / "plugins", state / "lumen", data):
        d.mkdir(parents=True, exist_ok=True)

    (home / ".bashrc").write_text(BASHRC)
    npm = home / "bin" / "npm"
    npm.write_text(NPM)
    npm.chmod(0o755)
    shutil.copy(ROOT / "examples" / "watch_pane.py", home / "bin" / "watch_pane.py")
    status = home / "bin" / "status"
    status.write_text("#!/bin/sh\ndate +%T\necho\ngit status --short\necho\ngit log --oneline -4\n")
    status.chmod(0o755)
    # `lumen` in the demo shell is this build, not whatever is installed.
    (home / "bin" / "lumen").symlink_to(LUMEN)

    # The shop project, with a history whose dates and authors never change.
    shop = home / "shop"
    shop.mkdir()
    git_env = dict(os.environ, HOME=str(home), GIT_CONFIG_NOSYSTEM="1")
    git = lambda *a, **kw: subprocess.run(["git", *a], cwd=shop, env=git_env, check=True, capture_output=True, **kw)
    git("init", "-q", "-b", "main")
    t = 1_730_000_000
    for i, (msg, who, path) in enumerate(COMMITS):
        f = shop / path
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(f"// {msg}\nexport const step = {i}\n")
        git("add", "-A")
        when = f"{t + i * 9_000} +0000"
        env = dict(git_env, GIT_AUTHOR_NAME=who, GIT_AUTHOR_EMAIL=f"{who.lower()}@shop.dev", GIT_COMMITTER_NAME=who,
                   GIT_COMMITTER_EMAIL=f"{who.lower()}@shop.dev", GIT_AUTHOR_DATE=when, GIT_COMMITTER_DATE=when)
        subprocess.run(["git", "commit", "-q", "-m", msg], cwd=shop, env=env, check=True)
    (shop / "src" / "checkout" / "totals.ts").write_text("// work in progress\n")
    (shop / "README.md").write_text("# shop\n")

    # Plugins: the examples the scenes use, approved as if you'd said yes.
    for name in ("todo",):
        shutil.copytree(ROOT / "examples" / "plugins" / name, config / "lumen" / "plugins" / name)
    approvals = {}
    for key, folder in [("builtin:pomodoro", ROOT / "plugins" / "pomodoro"),
                        ("todo", ROOT / "examples" / "plugins" / "todo")]:
        m = tomllib.loads((folder / "plugin.toml").read_text())
        cmd = m["command"].split() if isinstance(m["command"], str) else m["command"]
        approvals[key] = {"command": cmd, "permissions": m.get("permissions", []), "allowed": True}
    (state / "lumen" / "plugin-approvals.json").write_text(json.dumps(approvals, indent=2))
    todo_data = state / "lumen" / "plugin-data" / "todo"
    todo_data.mkdir(parents=True)
    (todo_data / "todo.json").write_text(json.dumps(TODOS, indent=1))

    return {
        "HOME": str(home),
        "USER": "you",
        "LOGNAME": "you",
        "SHELL": "/bin/bash",
        "XDG_CONFIG_HOME": str(config),
        "XDG_STATE_HOME": str(state),
        "XDG_DATA_HOME": str(data),
        "LANG": "C.UTF-8",
    }


def write_config(env: dict, theme: str = "Rosé Pine", plugins=("pomodoro",)):
    cfg = Path(env["XDG_CONFIG_HOME"]) / "lumen" / "config.toml"
    cfg.write_text(
        f'theme = "{theme}"\n'
        "font-size = 14\n"
        "scrollbar = true\n"
        "cursor-blink = false\n"
        "notify-after = 0\n"
        f"plugins = {json.dumps(list(plugins))}\n"
    )


# ---- driving Lumen ------------------------------------------------------------


class Lumen:
    """One Lumen on the virtual display: start it, script it, capture it."""

    def __init__(self, disp: str, env: dict, runtime: Path, size):
        self.disp, self.size = disp, size
        self.shop = str(Path(env["HOME"]) / "shop")
        self.env = dict(os.environ, **env, DISPLAY=disp, GPUI_X11_SCALE_FACTOR=str(SCALE),
                        VK_ICD_FILENAMES=LAVAPIPE, XDG_RUNTIME_DIR=str(runtime))
        for k in ("LUMEN_SOCKET", "LUMEN_PANE", "WAYLAND_DISPLAY"):
            self.env.pop(k, None)
        session = Path(env["XDG_STATE_HOME"]) / "lumen" / "session.json"
        session.unlink(missing_ok=True)  # every scene starts from one fresh tab
        self.runtime = runtime / "lumen"
        before = set(self.runtime.glob("*.sock")) if self.runtime.exists() else set()
        self.proc = subprocess.Popen([str(LUMEN)], cwd=env["HOME"], env=self.env,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.x = xdisplay.Display(disp)
        deadline = time.time() + 30
        self.sock = None
        while time.time() < deadline and not self.sock:
            new = set(self.runtime.glob("*.sock")) - before if self.runtime.exists() else set()
            self.sock = next(iter(new), None)
            time.sleep(0.2)
        if not self.sock:
            self.stop()
            sys.exit("Lumen didn't start on the virtual display (no socket after 30s)")
        self.env["LUMEN_SOCKET"] = str(self.sock)
        self.window = self._find_window()
        self.window.configure(width=size[0] * SCALE, height=size[1] * SCALE, x=0, y=0)
        self.x.sync()
        self.first = json.loads(self.msg("list", "--json"))["tabs"][0]["panes"][0]["id"]
        self.msg("set-title", "--pane", self.first, "shop")
        time.sleep(1.5)

    def _find_window(self):
        root = self.x.screen().root
        for _ in range(100):
            for w in root.query_tree().children:
                try:
                    cls = w.get_wm_class()
                except Exception:
                    cls = None
                if cls and "lumen" in cls and w.get_attributes().map_state == X.IsViewable:
                    return w
            time.sleep(0.1)
        sys.exit("found no Lumen window on the virtual display")

    def msg(self, *args) -> str:
        r = subprocess.run([str(LUMEN), "msg", *map(str, args)], env=self.env, capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(f"lumen msg {' '.join(map(str, args))}: {r.stderr.strip()}")
        return r.stdout.strip()

    def run(self, pane, command: str, wait=0.6):
        """Type a command into a pane's shell and press Enter."""
        self.msg("send-text", "--pane", pane, command + "\\n")
        time.sleep(wait)

    # Keys go in through XTEST, as if typed on a keyboard plugged into the
    # virtual display.
    def keys(self, combo: str, wait=0.35):
        names = combo.split("+")
        mods = {"ctrl": "Control_L", "shift": "Shift_L", "alt": "Alt_L"}
        codes = [self.x.keysym_to_keycode(XK.string_to_keysym(mods.get(n, n))) for n in names]
        self._focus()
        for c in codes:
            xtest.fake_input(self.x, X.KeyPress, c)
        for c in reversed(codes):
            xtest.fake_input(self.x, X.KeyRelease, c)
        self.x.sync()
        time.sleep(wait)

    def type(self, text: str, wait=0.4):
        names = {" ": "space", "-": "minus", ".": "period", "/": "slash", "'": "apostrophe"}
        for ch in text:
            self.keys(names.get(ch, ch), wait=0.03)
        time.sleep(wait)

    def _focus(self):
        g = self.window.get_geometry()
        xtest.fake_input(self.x, X.MotionNotify, x=g.x + g.width // 2, y=g.y + g.height // 2)
        self.window.set_input_focus(X.RevertToParent, X.CurrentTime)

    def shoot(self, path: Path, wait=0.8) -> Path:
        """Capture the window, save it framed at `path`, and return the raw
        capture (for scenes that crop more out of it)."""
        time.sleep(wait)
        raw = Path(tempfile.mkstemp(suffix=".png")[1])
        subprocess.run(["import", "-display", self.disp, "-window", hex(self.window.id), str(raw)], check=True)
        frame(Image.open(raw), path)
        return raw

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()


# ---- framing ------------------------------------------------------------------
# Every shot is presented the same way: the window with rounded corners and a
# hairline edge, casting a soft shadow on a sunset gradient, with room around
# it. The gradient runs top to bottom from lavender (near Rosé Pine's iris)
# through rose and coral to sand, the colours of the campfire's embers.

SUNSET = [
    (0.00, (212, 187, 254)),  # lavender  #d4bbfe
    (0.22, (210, 182, 248)),
    (0.36, (218, 181, 231)),  # mauve     #dab5e7
    (0.46, (232, 166, 181)),  # rose      #e8a6b5
    (0.54, (243, 156, 142)),  # coral     #f39c8e
    (0.62, (255, 153, 112)),  # orange    #ff9970
    (0.80, (242, 172, 107)),
    (1.00, (235, 201, 123)),  # sand      #ebc97b
]


# Film grain over the gradient: each pixel is lightened or darkened by up to
# GRAIN levels, the same amount in every channel, so the colour holds and
# only the texture changes. The noise is seeded, so a rerun gives the same
# files.
GRAIN = 6


def grain(img: Image.Image) -> Image.Image:
    """`img` with a fine monochrome noise added."""
    w, h = img.size
    rng = random.Random(f"{w}x{h}")
    noise = Image.frombytes("L", (w, h), rng.randbytes(w * h))
    span = 2 * GRAIN + 1
    up = noise.point(lambda v: max(0, v * span // 256 - GRAIN))
    down = noise.point(lambda v: max(0, GRAIN - v * span // 256))
    rgb = img.convert("RGB")
    rgb = ImageChops.add(rgb, Image.merge("RGB", (up,) * 3))
    rgb = ImageChops.subtract(rgb, Image.merge("RGB", (down,) * 3))
    return rgb.convert(img.mode)


def backdrop(w: int, h: int) -> Image.Image:
    """The sunset gradient from top to bottom, with a little grain."""
    column = Image.new("RGB", (1, h))
    px = column.load()
    for y in range(h):
        t = y / max(1, h - 1)
        for (t0, c0), (t1, c1) in zip(SUNSET, SUNSET[1:]):
            if t <= t1:
                f = (t - t0) / (t1 - t0)
                px[0, y] = tuple(round(a + (b - a) * f) for a, b in zip(c0, c1))
                break
    return grain(column.resize((w, h))).convert("RGBA")


def frame(shot: Image.Image, path: Path, pad: int = None, canvas=None):
    """Save `shot` as a framed image: rounded, edged, shadowed, on the backdrop.
    With `canvas` (width, height), the backdrop is that size and the shot sits
    in its middle."""
    shot = shot.convert("RGBA")
    w, h = shot.size
    radius = 12 * SCALE
    pad = pad if pad is not None else round(min(w, h) * 0.075)
    W, H = canvas or (w + 2 * pad, h + 2 * pad)
    px, py = (W - w) // 2, (H - h) // 2

    mask = Image.new("L", (w, h), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, w - 1, h - 1), radius, fill=255)
    shot.putalpha(mask)

    # Light theme windows get a dark edge; dark ones a light edge.
    luma = sum(shot.convert("L").resize((1, 1)).getdata()) / 255
    edge_colour = (0, 0, 0, 40) if luma > 0.5 else (255, 255, 255, 34)
    edge = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(edge).rounded_rectangle((0, 0, w - 1, h - 1), radius, outline=edge_colour, width=SCALE)

    out = backdrop(W, H)
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    lift = round(pad * 0.22)
    ImageDraw.Draw(shadow).rounded_rectangle((px, py + lift, px + w, py + h + lift), radius, fill=(60, 24, 40, 120))
    out.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(pad * 0.42)))
    out.alpha_composite(shot, (px, py))
    out.alpha_composite(edge, (px, py))
    path.parent.mkdir(parents=True, exist_ok=True)
    out.convert("RGB").save(path, optimize=True)
    print(f"  {path.relative_to(ROOT)}  {W}x{H}")


# ---- the scenes ---------------------------------------------------------------
# Each takes a running Lumen and the output folder.


def shop_workspace(l: Lumen, campfire=True):
    """The main tab: a shell with test runs (one failed), the dev server, and
    a campfire; plus three more tabs with badges a script or plugin set."""
    left = l.first
    l.msg("set-title", "--pane", left, "shop")
    l.run(left, "cd ~/shop && clear && git log --oneline -5", 0.8)
    l.run(left, "npm test", 0.8)
    l.run(left, "npm test -- totals", 0.8)
    l.run(left, "git status -sb", 0.8)
    right = l.msg("split", "right", "--pane", left, "--size", "0.42", "--cwd", l.shop, "--no-focus")
    l.run(right, "clear && npm run dev", 0.8)
    if campfire:
        l.msg("split", "down", "--pane", right, "--size", "0.48", "--run", "clear && lumen campfire", "--no-focus")
    tests = l.msg("new-tab", "--cwd", l.shop)
    l.msg("set-title", "--pane", tests, "tests")
    l.msg("tab-style", "--pane", tests, "--badge", "✓ 128", "--color", "green")
    deploy = l.msg("new-tab", "--cwd", l.shop)
    l.msg("set-title", "--pane", deploy, "deploy")
    l.msg("tab-style", "--pane", deploy, "--badge", "web-1.staging", "--color", "blue")
    notes = l.msg("new-tab", "--cwd", l.shop)
    l.msg("set-title", "--pane", notes, "notes")
    l.msg("tab-style", "--pane", notes, "--badge", "3 to do")
    l.msg("focus", left)
    return left, right


def scene_main(l, out):
    shop_workspace(l)
    time.sleep(6)  # let the fire catch and its key hint fade
    raw = l.shoot(ROOT / "assets" / "screenshot.png")
    # The tab bar alone, for the "tabs" tour slide, framed tightly.
    bar = Image.open(raw).crop((0, 0, 760 * SCALE, 44 * SCALE))
    frame(bar, out / "tabs.png", pad=28 * SCALE)
    # The same strip on a backdrop the size of the feature close-ups, for
    # places that show it among them (the website's tour).
    w, h = FEATURE[0] * SCALE, FEATURE[1] * SCALE
    pad = round(min(w, h) * 0.075)
    frame(bar, out / "tabs-tile.png", pad=pad, canvas=(w + 2 * pad, h + 2 * pad))
    raw.unlink()


def scene_palette(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && git log --oneline -12", 0.8)
    l.keys("ctrl+shift+p", 0.6)
    l.type("tab")
    l.shoot(out / "palette.png")


def scene_find(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && git log --format='%h %an  %s' --stat | head -40", 1.0)
    l.keys("ctrl+shift+f", 0.5)
    l.type("checkout")
    l.shoot(out / "find.png")


def scene_select(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && git log --oneline -4 && npm test", 1.0)
    l.keys("ctrl+shift+space", 0.5)
    for k in ("k", "k", "k", "k", "0"):  # up to the failed test
        l.keys(k, 0.1)
    l.keys("shift+v", 0.1)
    l.keys("j", 0.1)
    l.shoot(out / "select.png")


def scene_pomodoro(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && git log --oneline -12", 0.8)
    l.keys("ctrl+shift+p", 0.6)
    l.type("start pomodoro")
    l.keys("Return", 1.2)  # the plugin asks how long: 25 minutes is first
    l.keys("Return", 2.5)
    l.shoot(out / "pomodoro.png")


def scene_todo(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && git log --oneline -12", 0.8)
    l.keys("alt+t", 2.5)
    l.shoot(out / "todo.png")


def scene_gallery(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && git log --oneline -12", 0.8)
    l.keys("ctrl+shift+p", 0.6)
    l.type("plugin gallery")
    l.keys("Return", 1.0)
    l.shoot(out / "gallery.png")


def scene_watch(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && git log --oneline -6", 0.5)
    l.run(pane, 'PYTHONPATH="$(lumen python-path)" watch_pane.py -n 1 status', 3.5)
    l.shoot(out / "watch.png")


def scene_campfire(l, out):
    pane = l.first
    l.run(pane, "cd ~/shop && clear && npm test -- totals", 1.0)
    l.msg("split", "right", "--pane", pane, "--size", "0.5", "--run", "clear && lumen campfire", "--no-focus")
    time.sleep(6)  # past the key hint
    l.shoot(out / "campfire.png")


def scene_light(l, out):
    shop_workspace(l, campfire=False)
    l.shoot(out / "light.png", wait=1.5)


# name -> (what it shows, window size, theme, scene)
SCENES = {
    "main": ("tabs with badges, split panes, test runs with a failed mark, a campfire", MAIN, "Rosé Pine", scene_main),
    "palette": ("the command palette", FEATURE, "Rosé Pine", scene_palette),
    "find": ("find in scrollback", FEATURE, "Rosé Pine", scene_find),
    "select": ("keyboard select mode", FEATURE, "Rosé Pine", scene_select),
    "pomodoro": ("the Pomodoro Timer plugin", FEATURE, "Rosé Pine", scene_pomodoro),
    "todo": ("a todo list in a plugin pane", (960, 600), "Rosé Pine", scene_todo),
    "gallery": ("the Plugin Gallery", FEATURE, "Rosé Pine", scene_gallery),
    "watch": ("watch_pane.py, an example script", FEATURE, "Rosé Pine", scene_watch),
    "campfire": ("lumen campfire beside a shell", FEATURE, "Rosé Pine", scene_campfire),
    "light": ("the main layout in Rosé Pine Dawn, the light theme", MAIN, "Rosé Pine Dawn", scene_light),
}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("scenes", nargs="*", help="scenes to take (default: all)")
    ap.add_argument("--list", action="store_true", help="list the scenes and stop")
    ap.add_argument("--keep", action="store_true", help="keep the demo folder and print where it is")
    args = ap.parse_args()
    if args.list:
        for name, (what, *_rest) in SCENES.items():
            print(f"  {name:<9} {what}")
        return
    unknown = [s for s in args.scenes if s not in SCENES]
    if unknown:
        sys.exit(f"no such scene: {', '.join(unknown)} (see --list)")
    if not LUMEN.exists():
        sys.exit(f"{LUMEN.relative_to(ROOT)} is missing: run `cargo build --release` first")
    for tool in ("Xvfb", "import"):
        if not shutil.which(tool):
            sys.exit(f"{tool} is missing (apt install xvfb imagemagick)")
    if not Path(LAVAPIPE).exists():
        sys.exit("Mesa's lavapipe Vulkan driver is missing (apt install mesa-vulkan-drivers)")

    base = Path(tempfile.mkdtemp(prefix="lumen-shots-"))
    # Unix socket paths are short, so Lumen's runtime folder gets its own.
    runtime = Path(tempfile.mkdtemp(prefix="lsr-", dir="/tmp"))
    os.chmod(runtime, 0o700)
    env = make_demo(base)

    # A virtual display big enough for a 2x window; -displayfd picks a free one.
    rfd, wfd = os.pipe()
    xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(wfd), "-screen", "0", "3200x2000x24", "-nolisten", "tcp"],
                            pass_fds=[wfd], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.close(wfd)
    disp = ":" + os.read(rfd, 16).decode().strip()
    os.close(rfd)

    OUT.mkdir(parents=True, exist_ok=True)
    try:
        for name in args.scenes or SCENES:
            what, size, theme, scene = SCENES[name]
            print(f"{name}: {what}")
            write_config(env, theme)
            lumen = Lumen(disp, env, runtime, size)
            try:
                scene(lumen, OUT)
            finally:
                lumen.stop()
    finally:
        xvfb.terminate()
        shutil.rmtree(runtime, ignore_errors=True)
        if args.keep:
            print(f"demo folder kept: {base}")
        else:
            shutil.rmtree(base, ignore_errors=True)


if __name__ == "__main__":
    main()
