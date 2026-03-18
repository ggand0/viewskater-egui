# ViewSkater

A fast, cross-platform image viewer built with Rust and egui. Designed for exploring and comparing large sets of images. Linux, macOS and Windows are supported.

This is the egui port of the original [iced version](https://github.com/ggand0/viewskater).

## Features

- Dynamic image caching with background preloading
- Continuous image rendering via key presses and slider
- Dual pane view for side-by-side image comparison
- Scroll-to-zoom centered on cursor, click-drag to pan
- Fullscreen mode with cursor proximity UI reveal
- Supports jpg, png, bmp, webp, gif, tiff, qoi, tga

## Installation

Download the pre-built binaries from the [releases page](https://github.com/ggand0/viewskater-egui/releases), or build locally:

```bash
cargo run --release
```

To see debug logs:
```bash
RUST_LOG=viewskater_egui=debug cargo run --release
```

### Linux icon setup

On GNOME 46+ (Ubuntu 24.04+), the taskbar icon requires installing a `.desktop` file and icon:

```bash
mkdir -p ~/.local/share/icons/hicolor/256x256/apps
cp assets/icon_256.png ~/.local/share/icons/hicolor/256x256/apps/viewskater-egui.png
gtk-update-icon-cache -f ~/.local/share/icons/hicolor/
cp assets/viewskater-egui.desktop ~/.local/share/applications/
```

For the AppImage, update the `Exec=` line to point to the AppImage path:
```bash
sed -i "s|Exec=.*|Exec=/path/to/viewskater-egui.AppImage %f|" \
    ~/.local/share/applications/viewskater-egui.desktop
```

## Usage

Drag and drop an image or a directory of images onto a pane, and navigate through the images using the **A / D** keys or the slider.
Use the mouse wheel to zoom in/out of an image.

In dual-pane mode (**Ctrl+2** / **Cmd+2**), the slider syncs images in both panes by default.

## Shortcuts

On macOS, use **Cmd** instead of **Ctrl**.

| Input | Action |
|---|---|
| A / D or Left / Right arrow | Previous / next image |
| Hold A / D or arrows | Skate mode (continuous scroll) |
| Home / End | First / last image |
| Ctrl+1 / Ctrl+2 | Single / dual pane |
| Ctrl+O | Open file |
| Ctrl+Shift+O | Open folder |
| Ctrl+W | Close images |
| Ctrl+Q | Quit |
| Scroll wheel | Zoom (centered on cursor) |
| Click + drag | Pan |
| Double-click | Reset zoom and pan |
| F11 | Toggle fullscreen |
| Escape | Exit fullscreen |

## License

ViewSkater is licensed under either of
- Apache License, Version 2.0
  ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license
  ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
