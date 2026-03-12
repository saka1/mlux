# Bug: インラインコードの背景が次の行に重なる

**重要度**: Low — レンダリングの見た目の問題（コンパイルは成功する）
**発見**: README.md のビューア表示で目視確認
**ステータス**: **修正済み**

---

## 症状

インラインコード（`` `~/.config/mlux/config.toml` `` 等）を含む段落で、
インラインコードの背景矩形が次の視覚行のテキストと重なり、文字が読みにくくなる。

特に以下の条件で顕著:
- 段落が折り返されてインラインコードが行末付近にある
- インラインコードが複数含まれる行

---

## 原因: Typst の `inset` vs `outset`

テーマのインラインコードスタイル（修正前）:

```typst
#show raw.where(block: false): it => box(
  fill: rgb("#313244"), inset: (x: 4pt, y: 2pt), radius: 3pt,
  text(font: "DejaVu Sans Mono", size: 10pt, it))
```

### `inset` はレイアウトサイズを変える

Typst の `box` における `inset` は CSS の `padding` に相当する。
内部実装では `pad::grow()` がフレームのサイズ自体を拡大する:

```
box のレイアウト高さ = コンテンツ高さ + inset.top + inset.bottom
```

`inset: (y: 2pt)` の場合、box は上下に 2pt ずつ、計 4pt 高くなる。

### インライン要素の行高さへの影響

`box` はインライン要素としてテキスト行の中に配置される。
行の高さは**その行で最も高い要素**で決まるため、
`inset` で高くなった `box` があると、その行全体の高さが増加する。

一方、行間 (`leading`) は行の底辺から次の行の上辺までの固定距離:

```typst
#set par(leading: 1em)  // ≈ 12pt（本文 12pt の場合）
```

行が 4pt 高くなっても `leading` は変わらないので、
背景矩形の下端が次の行のテキスト領域に食い込む。

```
通常の行:     [  テキスト  ]
              ←── leading ──→
              [  テキスト  ]

inset あり:   [  テキスト  ]
              [ `コード` ↕+4pt ]  ← box が 4pt 高い
              ←── leading ──→     ← 同じ距離だが box の底辺から計測
              [  テ↑スト  ]       ← 背景がここに重なる
```

### `outset` はレイアウトに影響しない

`outset` は `fill_and_stroke()` で処理され、
**背景色やボーダーの描画位置だけ**を拡張する。
box のレイアウト上のサイズは変わらない:

```
box のレイアウト高さ = コンテンツ高さ（変化なし）
背景の描画高さ       = コンテンツ高さ + outset.top + outset.bottom
```

背景の見た目は `inset` と同一だが、行高さの計算には参入されないため、
次の行との距離が変わらない。

---

## 修正内容

`inset` の Y 成分を `outset` に移動:

```typst
// Before:
box(fill: ..., inset: (x: 4pt, y: 2pt), radius: 3pt, ...)

// After:
box(fill: ..., inset: (x: 4pt), outset: (y: 2pt), radius: 3pt, ...)
```

- `inset: (x: 4pt)` — 水平パディングはレイアウトに反映（テキストと box 端の余白が必要）
- `outset: (y: 2pt)` — 垂直方向は背景の塗りだけ拡張（行高さに影響しない）

### 対象ファイル

- `themes/catppuccin.typ` (line 20)
- `themes/catppuccin-latte.typ` (line 20)

### 回帰テスト

`tests/integration.rs` に `test_inline_code_no_line_overlap` を追加。
インラインコードを含む段落と含まない段落の行間が同一であることを検証する。

---

## Typst 内部実装メモ

Typst のソースコード上での処理の流れ:

1. **`inset`**: `BoxElem::layout()` → `pad::grow(regions, inset)` でレイアウト領域を縮小
   → コンテンツをレイアウト → `pad::grow(frame, inset)` でフレームを拡大。
   結果として **フレームサイズが `inset` 分だけ増加**する。

2. **`outset`**: `BoxElem::layout()` の最後で `fill_and_stroke()` を呼ぶ際、
   `outset` を渡す → 背景矩形の座標計算に `outset` が加算される。
   フレームサイズ自体は変わらない。

この区別は CSS の `padding`（レイアウト影響あり）と
`outline-offset`（レイアウト影響なし）の関係に近い。
ただし CSS には「背景だけ拡張する」直接的な対応物はないため、
Typst 特有の概念として理解する必要がある。
