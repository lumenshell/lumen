# xim-ctext, patched for Lumen

A copy of [xim-ctext 0.3.0](https://crates.io/crates/xim-ctext) (MIT, by
Riey; see `LICENSE`), used through `[patch.crates-io]` in Lumen's
`Cargo.toml`.

GPUI's X11 input-method client (`zed-xim`) decodes the text an input method
sends with `compound_text_to_utf8(..).expect(..)`. The original decoder
returned an error for plain Latin-1 text, which is what ibus sends when you
type "é" with a compose sequence or Ctrl+Shift+U, and for Chinese (GB 2312)
and Korean (KS C 5601) text, so typing any of those crashed the window.

The only change is `compound_text_to_utf8` in `src/lib.rs`: it now decodes
ASCII, Latin-1, UTF-8, JIS X 0208, GB 2312 and KS C 5601 segments and skips
anything else, and never returns an error. Drop this copy once GPUI's XIM
client handles decode errors itself.
