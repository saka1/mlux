#set page(width: 660pt, height: auto, margin: 40pt, fill: rgb("#1e1e2e"))
#set text(font: "Noto Sans JP", size: 12pt, fill: rgb("#cdd6f4"))
#set par(leading: 1em, justify: true, first-line-indent: 0pt)

// 見出し
#show heading.where(level: 1): it => block(below: 1em, above: 2.2em,
  text(24pt, weight: "bold", fill: rgb("#cba6f7"), it.body))
#show heading.where(level: 2): it => block(below: 1em, above: 1.9em,
  text(20pt, weight: "bold", fill: rgb("#f5c2e7"), it.body))
#show heading.where(level: 3): it => block(below: 1em, above: 1.6em,
  text(16pt, weight: "bold", fill: rgb("#f5e0dc"), it.body))

// コードブロック
#show raw.where(block: true): it => block(
  fill: rgb("#313244"), inset: 12pt, radius: 6pt, width: 100%,
  text(font: "DejaVu Sans Mono", size: 10pt, it))

// インラインコード
#show raw.where(block: false): it => box(
  fill: rgb("#313244"), inset: (x: 4pt, y: 2pt), radius: 3pt,
  text(font: "DejaVu Sans Mono", size: 10pt, it))

// リスト
#set list(marker: ([•], [‣], [–]), indent: 1em, body-indent: 0.7em)
#set enum(indent: 1em, body-indent: 0.7em)

// 引用ブロック
#show quote.where(block: true): it => block(
  inset: (left: 16pt, y: 8pt),
  stroke: (left: 3pt + rgb("#89b4fa")),
  text(fill: rgb("#a6adc8"), it.body))

// テーブル
#set table(stroke: 0.5pt + rgb("#585b70"), inset: 8pt,
  fill: (_, y) => if y == 0 { rgb("#313244") } else { none })

// リンク
#show link: it => text(fill: rgb("#89b4fa"), underline(it))

// 強調（bold）: 本文と同色、太さのみ変える
#show strong: set text(fill: rgb("#cdd6f4"))

// 斜体: Pink
#show emph: set text(fill: rgb("#f5c2e7"))

// 打ち消し線: Subtext 1 のストローク
#show strike: set strike(stroke: 1pt + rgb("#a6adc8"))

// 見出し h4-h6
#show heading.where(level: 4): it => block(below: 1em, above: 1.3em,
  text(14pt, weight: "bold", fill: rgb("#eba0ac"), it.body))
#show heading.where(level: 5): it => block(below: 1em, above: 1.2em,
  text(13pt, weight: "bold", fill: rgb("#f9e2af"), it.body))
#show heading.where(level: 6): it => block(below: 1em, above: 1.0em,
  text(12pt, weight: "bold", fill: rgb("#a6adc8"), it.body))

// 数式: 本文フォントとのバランス調整
#show math.equation: set text(font: "STIX Two Math", size: 13pt)

// 水平線: Surface 2
#show line: set line(stroke: 1pt + rgb("#585b70"))

// コードブロック: Catppuccin Mocha シンタックスハイライト
#set raw(theme: "catppuccin-mocha.tmTheme")
