# mlux

[![CI](https://github.com/saka1/mlux/actions/workflows/ci.yml/badge.svg)](https://github.com/saka1/mlux/actions/workflows/ci.yml)

A Markdown viewer for modern terminals, with Typst-quality typesetting.

<img src="docs/ss.png" alt="mlux terminal viewer" width="700">

## Features

**Typst-powered typesetting** -- Headings, code blocks, tables, and
blockquotes rendered through Typst's typesetting engine. Local images,
LaTeX math (`$...$`, `$$...$$`), and fenced `mermaid` code blocks are
all displayed inline. Git diff markers annotate lines changed since the
last commit.

**Vim-inspired terminal viewer** -- Scrolling, regex search with highlights,
table of contents, link navigation, and URL picker.

Also includes file watching (`--watch`), automatic dark / light detection,
PNG export (`mlux render`), and stdin support
(`cat README.md | mlux -`).

## Requirements

- **Platform:** macOS, Linux, or WSL
- **Terminal:** [Ghostty](https://ghostty.org/),
  [Kitty](https://sw.kovidgoyal.net/kitty/), or another terminal that
  supports the
  [Kitty Graphics Protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)

## Installation

Pre-built binaries are available on the
[Releases](https://github.com/saka1/mlux/releases) page.
They bundle additional fonts (including Japanese) and are the recommended
way to install mlux.

Alternatively, install a minimal build via Cargo (basic Latin fonts only):

```console
cargo install mlux
```

## Usage

```console
# View in terminal
mlux input.md

# Watch for changes
mlux -w input.md

# Fetch and display remote images
mlux --allow-remote-images input.md

# Pipe from stdin
cat README.md | mlux -

# Export to PNG
mlux render input.md -o output.png
mlux render --scale=1.5 input.md -o output.png

# Debug logging
mlux --log /tmp/mlux.log input.md
mlux --debug input.md
```

## Keybindings

| Key | Action |
|-----|--------|
| `j` / `k` | Scroll down / up |
| `d` / `u` | Half-page down / up |
| `g` / `G` | Jump to top / bottom |
| `[N]g` / `[N]G` | Jump to line N |
| `/` / `?` | Search forward / backward (regex) |
| `n` / `N` | Next / previous match |
| `:grep` | Full-screen search picker |
| `[N]o` | Open link on line N |
| `O` | URL picker (all URLs) |
| `Ctrl-O` | Pop jump stack |
| `[N]y` / `[N]Y` | Yank line / block N |
| `t` | Table of contents |
| `+` / `-` / `=` | Zoom in / out / reset |
| `:` | Command mode (`:q` `:reload` `:grep` `:open` `:back` `:log` `:noh`) |
| `q` | Quit |

`/` and `?` accept regex patterns. Press Enter to confirm, then navigate
matches with `n` / `N`. `:noh` clears highlights.

`[N]o` opens the link on line N. External URLs open in a browser.
Links to local `.md` files navigate inline, and `Ctrl-O` pops back
to the previous location with scroll position restored.

### Experimental presets

`--exp-preset=adaptive` enables an experimental scroll behavior that
adjusts step size to input frequency and uses kinetic (momentum)
interpolation. The exact behavior may change between releases.

## How it works

mlux converts Markdown to Typst markup, then renders each page as a PNG
image. The terminal viewer displays pages via the Kitty Graphics Protocol
using tile-based lazy rendering -- only tiles visible in the viewport are
rendered on demand, keeping memory usage and latency low.

mlux does not execute arbitrary code, but it does process complex input
through its rendering pipeline. Exercise caution with untrusted documents.

## Gallery

<img src="docs/gallery01.png" alt="Rendering gallery (light theme)" width="300">
<img src="docs/gallery02.png" alt="Rendering gallery (dark theme)" width="300">

## License

MIT

