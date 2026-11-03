# lumen_plugin

Write plugins and scripts for [Lumen](../README.md) in Python. Standard library
only, Python 3.8 or newer.

Plugins don't need to install anything: Lumen puts this package on every
plugin's `PYTHONPATH`. Scripts can use the copy Lumen keeps:

```sh
PYTHONPATH="$(lumen python-path)" python3 my_script.py
```

or install it into a virtualenv with `pip install ./python`.

The guide and reference are in [docs/plugins.md](../docs/plugins.md).

Tests: `python3 -m unittest discover -s python/tests`
