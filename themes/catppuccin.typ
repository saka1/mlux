#set page(width: 660pt, height: auto, margin: 40pt, fill: rgb("#1e1e2e"))
#set text(font: "Noto Sans JP", size: 12pt, fill: rgb("#cdd6f4"))
#set par(leading: 1em, justify: true, first-line-indent: 0pt)

// 見出し (above: 1.4em, below: 0.9em — em はフォントサイズ基準)
#show heading.where(level: 1): it => text(24pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#cba6f7"), it.body)))
#show heading.where(level: 2): it => text(20pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#f5c2e7"), it.body)))
#show heading.where(level: 3): it => text(16pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#f5e0dc"), it.body)))

// コードブロック
#show raw.where(block: true): it => block(
  fill: rgb("#313244"), inset: 12pt, radius: 6pt, width: 100%,
  text(font: "DejaVu Sans Mono", size: 10pt, it))

// インラインコード (raw の ShowSet が size: 0.8em を適用するため / 0.8 で補正)
#show raw.where(block: false): it => box(
  fill: rgb("#313244"), inset: (x: 0.3em / 0.8), outset: (y: 0.15em / 0.8), radius: 3pt,
  text(font: "DejaVu Sans Mono", size: 0.85em / 0.8, it))

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
#show heading.where(level: 4): it => text(14pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#eba0ac"), it.body)))
#show heading.where(level: 5): it => text(13pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#f9e2af"), it.body)))
#show heading.where(level: 6): it => text(12pt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: rgb("#a6adc8"), it.body)))

// 数式: 本文フォントとのバランス調整
#show math.equation: set text(font: "STIX Two Math", size: 13pt)

// 水平線: Surface 2
#show line: set line(stroke: 1pt + rgb("#585b70"))

// 画像プレースホルダー: Surface 2 ボーダー
#let image-placeholder(path) = block(stroke: 0.5pt + rgb("#585b70"), inset: 8pt, radius: 4pt)[Image: #path]

// コードブロック: Catppuccin Mocha シンタックスハイライト
#set raw(theme: "catppuccin-mocha.tmTheme")
