# Lumen's icon

- `linux/hicolor/`: the icon theme tree (16–512 px and scalable SVG), installed
  by `scripts/install-linux.sh` into `~/.local/share/icons/hicolor/`. The menu
  entry (`assets/linux/lumen.desktop`) refers to it as `Icon=lumen`.
- `linux/lumen-1024.png`: for AppImage, Flatpak and Snap metadata.
- `macos/Lumen.icns`: copied into `Lumen.app/Contents/Resources/` by
  `scripts/bundle-macos.sh` (`CFBundleIconFile` = `Lumen`).
- `macos/lumen-1024.png`: on Apple's icon grid (824 px body, transparent margin).
- `source/`: the SVG masters, macOS and Linux, full and small. Sizes 32 px and
  below use the small version (a bolder prompt, no shadow or fine crease line).
