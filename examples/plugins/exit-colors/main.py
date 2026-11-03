"""Tint each tab by how its last command went: green when it worked, red
when it failed (with the exit status as a badge), and nothing while a
command runs. Needs shell integration, which reports commands.

Tab styles are the plugin's to set, so this doesn't touch tab titles and
works alongside Build Status.
"""

from lumen_plugin import LumenError, Plugin

app = Plugin()


@app.on("command-started")
async def started(event):
    try:
        await event.pane.set_tab_style(badge=None, color=None)
    except LumenError:
        pass  # closed in the meantime


@app.on("command-finished")
async def finished(event):
    code = event.get("exit")
    if code is None:
        return
    # 130 is Ctrl+C: you stopped it, it didn't fail.
    failed = code not in (0, 130)
    style = {"badge": f"exit {code}", "color": "red"} if failed else {"badge": None, "color": "green"}
    try:
        await event.pane.set_tab_style(**style)
    except LumenError:
        pass


app.run()
