# mlux
A rich Markdown renderer for modern terminals,
powered by Rust and [Typst](https://typst.app/).

## How it works

Modern terminals like [Kitty](https://sw.kovidgoyal.net/kitty/) and [Ghostty](https://ghostty.org/) that support the
[Kitty Graphics Protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) can display
images inline. mlux takes advantage of this: it parses Markdown, converts it to Typst markup, and renders high-quality typeset output as a PNG image.
The built-in terminal viewer then displays the rendered document directly in your terminal.

## Features

- **Beautiful output** -- Typst's typesetting engine handles line breaking, paragraph
  spacing, headings, code blocks, tables, blockquotes, and more.
- **Terminal viewer** -- View rendered Markdown directly in your terminal with
  pixel-precise line numbers, vim-style scrolling, and tile-based lazy rendering.
  Requires a terminal that supports the Kitty Graphics Protocol (e.g. Ghostty, Kitty).
- **Heading picker** -- Press `/` to open an interactive picker that lists headings and jumps to your selection.
- **File watching** -- Automatically re-renders when the source file changes.

## Usage

```console
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
| `[N]g` / `[N]G` | Jump to line N |
| `/` | Search headings |
| `n` / `N` | Next / previous search match |
| `y` / `Y` | Yank line / block |
| `q` | Quit |

## Building

```
cargo build --release
```

Requires Rust 1.85+ (edition 2024).

