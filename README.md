# viewskater-egui

An egui reimplementation of [viewskater](https://github.com/ggand0/viewskater).

## Features

- Dual pane mode with synced navigation
- Drag-and-drop files/folders onto panes
- Keyboard and slider navigation with natural sort ordering
- Skate mode for continuous scrolling through images
- Scroll-to-zoom centered on cursor, click-drag to pan, double-click to reset
- Sliding window cache with background preloading and LRU decode cache
- Supports jpg, png, bmp, webp, gif, tiff, qoi, tga

## Usage

```bash
cargo run --profile opt-dev -- /path/to/images/
cargo run --profile opt-dev -- /path/to/image.jpg
cargo run --profile opt-dev  # launch empty, drag-and-drop to open
```

Set `RUST_LOG=viewskater_egui=debug` for debug logging.

## Controls

| Input | Action |
|---|---|
| A / D or Left / Right arrow | Previous / next image |
| Hold A / D or arrows | Skate mode (continuous scroll) |
| Home / End | First / last image |
| Ctrl+1 / Ctrl+2 | Single / dual pane |
| Scroll wheel | Zoom (centered on cursor) |
| Click + drag | Pan |
| Double-click | Reset zoom and pan |
| Slider | Jump to position |
