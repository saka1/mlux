#set page(width: 660pt, height: auto, margin: 40pt, fill: rgb("#1e1e2e"))
#set text(font: ("IPAGothic", "Noto Sans CJK JP", "Noto Sans"), size: 12pt, fill: rgb("#cdd6f4"))
#set par(leading: 1em, justify: true, first-line-indent: 0pt)

// 見出し
#show heading.where(level: 1): it => block(below: 1.2em, above: 1.8em,
  text(24pt, weight: "bold", fill: rgb("#cba6f7"), it.body))
#show heading.where(level: 2): it => block(below: 1.0em, above: 1.5em,
  text(20pt, weight: "bold", fill: rgb("#f5c2e7"), it.body))
#show heading.where(level: 3): it => block(below: 0.8em, above: 1.2em,
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
