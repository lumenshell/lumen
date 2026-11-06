# Lumen's own commands

Besides opening a window, `lumen` has two commands to run in a pane.

- [System info: `lumen info`](#system-info)
- [Campfire: `lumen campfire`](#campfire)

## System info

`lumen info` prints a flame in your theme's colours beside a summary of your
system:

- OS, kernel, uptime, shell, CPU, GPU, memory and public IP
- Lumen's version and window size, theme, font, whether shell integration is
  on, and which plugins are set up
- the terminal's 16 colours, underneath

It takes a few milliseconds and needs no other programs. GPU names come from
`lspci` or NVIDIA's driver when they're there.

### The public IP

The public IP comes from [api.ipify.org](https://www.ipify.org), over plain
HTTP. It's shown only if the answer is an IP address. Everything else prints
at once, and the IP fills in when it arrives, or reads "unavailable" when
you're offline. `--no-ip` skips it, so nothing leaves your machine.

### The flame

In a terminal, the flame burns until you press any key. The key still reaches
your shell, and Ctrl+C works too.

| Option | Does |
|---|---|
| `--brief` | Burns for two seconds, then settles |
| `--still` | Doesn't animate at all |
| `--no-ip` | Skips the public IP |

`--brief` suits your shell's startup. Add `lumen info --brief` to your
`~/.bashrc`, `~/.zshrc` or `config.fish` to see it in every new pane.

It also doesn't animate when the output wouldn't fit on screen. Piped,
redirected, or with `NO_COLOR=1`, it prints plain text.

## Campfire

`lumen campfire` fills the pane with a pixel-art campfire, for cold coding
sessions. Tongues of flame lick up from a bed of glowing coals under crossed
logs, in a ring of stones. Sparks and smoke drift up past a pine treeline and
the stars. On a light theme, it's daytime.

It's drawn from a fixed palette over your theme's background, with a pool of
firelight on the ground. The fire sizes itself to the pane and follows when
you resize. It runs on the alternate screen, so your scrollback stays as it
was.

| Key | |
|---|---|
| `↑` `↓` (or `+` `-`) | Stoke the fire up, or let it die down |
| `←` `→` | Change the wind |
| `Space` | Poke it, for a flare-up and a burst of sparks |
| `q`, `Esc` or `Ctrl+C` | Put it out |

To keep one burning beside your work:

```sh
lumen msg split right --size 0.3 --run 'lumen campfire' --no-focus
```
