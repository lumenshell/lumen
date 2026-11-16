# Publishing a plugin

Plugins are published through the registry,
[lumenshell/plugins](https://github.com/lumenshell/plugins): one small file per
plugin, added by pull request. Once it's merged, anyone can install yours:

```sh
lumen plugins install your-name/your-plugin
```

## 1. Make it work locally

Write it in a folder under `~/.config/lumen/plugins/` (see the
[plugin guide](plugins.md)), and use it for a while. Then check it the way the
registry will:

```sh
lumen plugins check ~/.config/lumen/plugins/your-plugin
```

It prints `ok`, the version and the permissions, or what's wrong.

## 2. Put it on GitHub

Push the plugin's folder (`plugin.toml` and its files) to a public GitHub
repository. It can be the whole repository or one folder in it.

Plugins run from their source as downloaded: there's no build step, so no
compiled files. Start it with something every machine has, such as `python3`,
`bash`, `sh` or `node`. Python plugins can use the standard library and
`lumen_plugin`.

Give `plugin.toml` a `version` like `1.0.0`, and commit.

## 3. Add the entry

In a fork of [lumenshell/plugins](https://github.com/lumenshell/plugins), add
`plugins/YOUR-GITHUB-NAME/PLUGIN-NAME.toml`:

```toml
repo = "https://github.com/you/your-plugin"
path = ""                       # the folder holding plugin.toml; "" for the root
description = "One sentence on what it does"
license = "MIT"
categories = ["git"]            # see categories.txt there

[[versions]]
version = "1.0.0"               # as in plugin.toml
commit = "…"                    # the full 40-character commit
permissions = ["input"]         # as in plugin.toml
lumen = "1.3.0"                 # the oldest Lumen it works with
```

Open a pull request. Its checks make sure you own the name (your GitHub
account, or an organisation you're a public member of), download your
repository at `commit`, and run `lumen plugins check` on it: the version and
permissions must match the entry. A maintainer then reviews and merges it.

## 4. New versions

Add a `[[versions]]` at the end and leave the others as they are: a published
version never changes. People run `lumen plugins update` to get it. If it asks
for a permission the previous version didn't, they're asked again.
