#set page(width: 660pt, height: auto, margin: 40pt, fill: rgb("#eff1f5"))
#set text(font: "Noto Sans JP", size: 12pt, fill: rgb("#4c4f69"),
  lang: "ja", cjk-latin-spacing: auto)
// Leading: JLReq §3.2.3 says line gap = 50-100% of em, and "one em spacing or
// close to it is more appropriate when the line length is longer than 35
// characters". The kihon-hanmen here is 580pt / 12pt ≈ 48 chars, so ~1em is
// indicated; 1.0em felt slightly slack, so we take "close to it" as 0.9em.
// Typst's par.leading is the inter-line gap only → 0.9em ≈ 190% line-height.
#set par(leading: 0.9em, justify: true, first-line-indent: 0pt)

// Headings (above: 1.4em, below: 0.9em — em is relative to font size)
#show heading.where(level: 1): it => text(24pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#8839ef"), it.body)))
#show heading.where(level: 2): it => text(21pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#ea76cb"), it.body)))
#show heading.where(level: 3): it => text(18pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#7287fd"), it.body)))

// Code block
#show raw.where(block: true): it => block(
  fill: rgb("#e6e9ef"), inset: 12pt, radius: 6pt, width: 100%,
  text(font: ("DejaVu Sans Mono", "Noto Sans JP"), size: 10pt, it))

// Inline code (raw's ShowSet applies size: 0.8em, so divide by 0.8 to compensate)
#show raw.where(block: false): it => box(
  fill: rgb("#e6e9ef"), inset: (x: 0.3em / 0.8), outset: (y: 0.15em / 0.8), radius: 3pt,
  text(font: ("DejaVu Sans Mono", "Noto Sans JP"), size: 0.85em / 0.8, it))

// Lists
#set list(marker: ([•], [‣], [–]), indent: 1em, body-indent: 0.7em)
#set enum(indent: 1em, body-indent: 0.7em)

// Block quote
#show quote.where(block: true): it => block(
  inset: (left: 16pt, y: 8pt),
  stroke: (left: 3pt + rgb("#1e66f5")),
  text(fill: rgb("#6c6f85"), it.body))

// Tables
#set table(stroke: 0.5pt + rgb("#acb0be"), inset: 8pt,
  fill: (_, y) => if y == 0 { rgb("#ccd0da") } else { none })

// Links
#show link: it => text(fill: rgb("#1e66f5"), underline(it))

// Strong (bold): same color as body, only weight differs
#show strong: set text(fill: rgb("#4c4f69"))

// Italic: Teal — cool tones recede, matching italic's subdued emphasis
#show emph: set text(fill: rgb("#179299"))

// Strikethrough: Subtext 1 stroke
#show strike: set strike(stroke: 1pt + rgb("#5c5f77"))

// Headings h4-h6
#show heading.where(level: 4): it => text(16pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#e64553"), it.body)))
#show heading.where(level: 5): it => text(14pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#df8e1d"), it.body)))
#show heading.where(level: 6): it => text(12pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#5c5f77"), it.body)))

// Math: tuned to balance with body font
#show math.equation: set text(font: "STIX Two Math", size: 13pt)

// Horizontal rule: Surface 2
#show line: set line(stroke: 1pt + rgb("#acb0be"))

// Image placeholder: Surface 2 border
#let image-placeholder(path) = block(stroke: 0.5pt + rgb("#acb0be"), inset: 8pt, radius: 4pt)[Image: #path]

// Code block: Catppuccin Latte syntax highlighting
#set raw(theme: "catppuccin-latte.tmTheme")
