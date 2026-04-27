#set page(width: 660pt, height: auto, margin: 40pt, fill: rgb("#1e1e2e"))
#set text(font: "Fira Sans", size: 12pt * scale, fill: rgb("#cdd6f4"), lang: "en")
// Leading: 0.75em (~169% line-height). Butterick's 120-145% is print-oriented
// and reads tight on screen (KGP @ 144 PPI); Fira Sans's tall x-height also
// pushes us toward looser values. Keeps visual contrast small against the CJK
// theme (catppuccin.typ) which uses 0.9em (~190%).
#set par(leading: 0.75em, justify: true, first-line-indent: 0pt)

// Headings (above: 1.4em, below: 0.9em — em is relative to font size)
#show heading.where(level: 1): it => text(24pt * scale,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#cba6f7"), it.body)))
#show heading.where(level: 2): it => text(21pt * scale,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#f5c2e7"), it.body)))
#show heading.where(level: 3): it => text(18pt * scale,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#b4befe"), it.body)))

// Code block
#show raw.where(block: true): it => block(
  fill: rgb("#313244"), inset: 12pt * scale, radius: 6pt * scale, width: 100%,
  text(font: "Fira Mono", size: 10pt * scale, it))

// Inline code (raw's ShowSet applies size: 0.8em, so divide by 0.8 to compensate)
#show raw.where(block: false): it => box(
  fill: rgb("#313244"), inset: (x: 0.3em / 0.8), outset: (y: 0.15em / 0.8), radius: 3pt * scale,
  text(font: "Fira Mono", size: 0.85em / 0.8, it))

// Lists
#set list(marker: ([•], [‣], [–]), indent: 1em, body-indent: 0.7em)
#set enum(indent: 1em, body-indent: 0.7em)

// Block quote
#show quote.where(block: true): it => block(
  inset: (left: 16pt * scale, y: 8pt * scale),
  stroke: (left: 3pt * scale + rgb("#89b4fa")),
  text(fill: rgb("#a6adc8"), it.body))

// Tables
#set table(stroke: 0.5pt * scale + rgb("#585b70"), inset: 8pt * scale,
  fill: (_, y) => if y == 0 { rgb("#313244") } else { none })

// Links
#show link: it => text(fill: rgb("#89b4fa"), underline(it))

// Strong (bold): same color as body, only weight differs
#show strong: set text(fill: rgb("#cdd6f4"))

// Strikethrough: Subtext 1 stroke
#show strike: set strike(stroke: 1pt * scale + rgb("#a6adc8"))

// Headings h4-h6
#show heading.where(level: 4): it => text(16pt * scale,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#eba0ac"), it.body)))
#show heading.where(level: 5): it => text(14pt * scale,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#f9e2af"), it.body)))
#show heading.where(level: 6): it => text(12pt * scale,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#a6adc8"), it.body)))

// Math: tuned to balance with body font
#show math.equation: set text(font: "STIX Two Math", size: 13pt * scale)

// Horizontal rule: Surface 2
#show line: set line(stroke: 1pt * scale + rgb("#585b70"))

// Image placeholder: Surface 2 border
#let image-placeholder(path) = block(stroke: 0.5pt * scale + rgb("#585b70"), inset: 8pt * scale, radius: 4pt * scale)[Image: #path]

// Code block: Catppuccin Mocha syntax highlighting
#set raw(theme: "catppuccin-mocha.tmTheme")
