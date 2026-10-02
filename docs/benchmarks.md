# Benchmarks

Three benchmark modes are built into the binary. Each opens the folder, runs by itself, prints a report and exits.

## Quick start

```bash
# skate through a folder and back
cargo run --profile opt-dev -- /path/to/folder --bench-nav

# drag the slider through it
cargo run --profile opt-dev -- /path/to/folder --bench-slider

# both, 3 runs each, two folders, reports written to benchmarks/
cargo run --profile opt-dev -- --bench-nav --bench-slider \
    --bench-dir /path/to/a --bench-dir /path/to/b \
    --bench-runs 3 --bench-out benchmarks
```

Use `--profile opt-dev` or `--release`. `RUST_LOG=viewskater_egui=info` shows the log around the report.

## Modes

| Flag | What it does |
|---|---|
| `--bench-nav` | Skates to the last image and back. |
| `--bench-slider` | Drags the slider from end to end and back, drags back and forth at 5 points, clicks at 20 points. |
| `--bench-preview` | Hovers over the slider so the preview thumbnails appear. |

They combine. One run does nav, then slider, then preview, on the same folder.

## Flags

| Flag | Default | Meaning |
|---|---|---|
| `--bench-dir DIR` | positional path | folder to benchmark, repeat for several |
| `--bench-runs N` | 1 | repeat everything N times, folder reopened between runs |
| `--bench-out DIR` | none | write a JSON and a markdown report per run plus a summary |
| `--bench-label TEXT` | none | free text in the report header, e.g. `before-exif` |
| `--bench-max-images N` | whole folder | nav: only the first N images and back |
| `--bench-tap-steps N` | 0 | nav: N single key presses after the two passes, timing press to image |
| `--bench-tap-rate PER_SEC` | 6 | nav: presses per second for that |
| `--bench-sweep-secs SECS` | 4 | slider: seconds per pass across the slider |
| `--bench-scrub-anchors N` | 5 | slider: points to drag back and forth at, 0 skips |
| `--bench-scrub-span SHARE` | 0.1 | slider: width of that motion as a share of the slider |
| `--bench-scrub-passes N` | 2 | slider: back-and-forth passes per point |
| `--bench-scrub-secs SECS` | 2 | slider: seconds per point |
| `--bench-jumps N` | 20 | slider: number of clicks, 0 skips |
| `--bench-skip PHASE,...` | none | leave out `skate-left`, `sweep`, `scrub` or `jump` |

## Output

The report is printed to the terminal. With `--bench-out DIR`, each run writes `DIR/<date>_<machine>_<folder>_run<n>.json` and `.md`, and the last run writes a `_summary.json` and `.md` with the mean and range over all runs. Headers record the version, commit, folder, image count and the cache and thread settings. `benchmarks/` is gitignored.

Nav reports images per second, frames stuck waiting for a decode, frame and decode times, CPU and memory. Slider reports the time each load blocked the UI, cache hits, time from release until the neighbouring images are reloaded, and for clicks the time until the clicked image appears.
