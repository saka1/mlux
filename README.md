# mlux

Markdown to PNG renderer powered by Rust and [Typst](https://typst.app/).

mlux parses Markdown with pulldown-cmark, converts it to Typst markup, and renders
high-quality typeset output as a PNG image. It also includes a built-in terminal viewer
that displays rendered documents inline using the Kitty Graphics Protocol.

## Features

- **Beautiful output** -- Typst's typesetting engine handles line breaking, paragraph
  spacing, headings, code blocks, tables, blockquotes, and more.
- **CJK support** -- Japanese text renders correctly out of the box (IPAGothic / Noto Sans CJK JP).
- **Themeable** -- Themes are plain Typst files. Ships with a Catppuccin Mocha dark theme.
- **Terminal viewer** -- View rendered Markdown directly in your terminal with
  pixel-precise line numbers, vim-style scrolling, and tile-based lazy rendering
  for constant memory usage regardless of document size.
  Requires a terminal that supports the Kitty Graphics Protocol (e.g. Ghostty, Kitty).

## Usage

```
# View in terminal (default)
mlux input.md

# Render to PNG
mlux render input.md -o output.png

# Custom width, resolution, and theme
mlux render input.md -o output.png --width 800 --ppi 144 --theme catppuccin
```

### Viewer keybindings

| Key | Action |
|-----|--------|
| `j` / `k` | Scroll down / up |
| `d` / `u` | Half-page down / up |
| `g` / `G` | Jump to top / bottom |
| `q` | Quit |

## Building

```
cargo build --release
```

Requires Rust 1.85+ (edition 2024).
