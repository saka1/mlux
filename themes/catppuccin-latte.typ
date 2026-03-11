#set page(width: 660pt, height: auto, margin: 40pt, fill: rgb("#eff1f5"))
#set text(font: "Noto Sans JP", size: 12pt, fill: rgb("#4c4f69"))
#set par(leading: 1em, justify: true, first-line-indent: 0pt)

// 見出し
#show heading.where(level: 1): it => block(below: 1em, above: 2.2em,
  text(24pt, weight: "bold", fill: rgb("#8839ef"), it.body))
#show heading.where(level: 2): it => block(below: 1em, above: 1.9em,
  text(20pt, weight: "bold", fill: rgb("#ea76cb"), it.body))
#show heading.where(level: 3): it => block(below: 1em, above: 1.6em,
  text(16pt, weight: "bold", fill: rgb("#dc8a78"), it.body))

// コードブロック
#show raw.where(block: true): it => block(
  fill: rgb("#e6e9ef"), inset: 12pt, radius: 6pt, width: 100%,
  text(font: "DejaVu Sans Mono", size: 10pt, it))

// インラインコード
#show raw.where(block: false): it => box(
  fill: rgb("#e6e9ef"), inset: (x: 4pt, y: 2pt), radius: 3pt,
  text(font: "DejaVu Sans Mono", size: 10pt, it))

// リスト
#set list(marker: ([•], [‣], [–]), indent: 1em, body-indent: 0.7em)
#set enum(indent: 1em, body-indent: 0.7em)

// 引用ブロック
#show quote.where(block: true): it => block(
  inset: (left: 16pt, y: 8pt),
  stroke: (left: 3pt + rgb("#1e66f5")),
  text(fill: rgb("#6c6f85"), it.body))

// テーブル
#set table(stroke: 0.5pt + rgb("#acb0be"), inset: 8pt,
  fill: (_, y) => if y == 0 { rgb("#ccd0da") } else { none })

// リンク
#show link: it => text(fill: rgb("#1e66f5"), underline(it))

// 強調（bold）: 本文と同色、太さのみ変える
#show strong: set text(fill: rgb("#4c4f69"))

// 斜体: Pink
#show emph: set text(fill: rgb("#ea76cb"))

// 打ち消し線: Subtext 1 のストローク
#show strike: set strike(stroke: 1pt + rgb("#5c5f77"))

// 見出し h4-h6
#show heading.where(level: 4): it => block(below: 1em, above: 1.3em,
  text(14pt, weight: "bold", fill: rgb("#e64553"), it.body))
#show heading.where(level: 5): it => block(below: 1em, above: 1.2em,
  text(13pt, weight: "bold", fill: rgb("#df8e1d"), it.body))
#show heading.where(level: 6): it => block(below: 1em, above: 1.0em,
  text(12pt, weight: "bold", fill: rgb("#5c5f77"), it.body))

// 数式: 本文フォントとのバランス調整
#show math.equation: set text(font: "STIX Two Math", size: 13pt)

// 水平線: Surface 2
#show line: set line(stroke: 1pt + rgb("#acb0be"))

// コードブロック: Catppuccin Latte シンタックスハイライト
#set raw(theme: "catppuccin-latte.tmTheme")
