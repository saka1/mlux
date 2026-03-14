# Known Bugs (Fuzz Testing)

`cargo +nightly-2025-11-01 fuzz run fuzz_pipeline` および `fuzz_convert` で発見されたバグ。

---

## ~~Bug 1: 空URLのリンクでTypstコンパイルエラー~~ (修正済み)

**重要度**: High — コンパイルが失敗する
**発見**: fuzz_pipeline (fuzz-3.log)
**アーティファクト**: `fuzz/artifacts/fuzz_pipeline/crash-377b47f45eb8f820819bd15e246740d14eda4a61`
**ステータス**: **修正済み** — 空URLリンクをプレーンテキストとして出力するように変更。テスト `test_link_empty_url` 追加。

### 再現入力

```markdown
[link]()
```

### 修正内容

`markup.rs` の `Event::Start(Tag::Link)` / `Event::End(TagEnd::Link)` ハンドラで
URLが空の場合は `#link("")[...]` を出力せず、リンクテキストをプレーンテキストとして出力する。

---

## ~~Bug 2: 深いブロック引用ネストでTypst show ruleの深度上限超過~~ (修正済み)

**重要度**: High — コンパイルが失敗する
**発見**: fuzz_pipeline (fuzz-0.log, fuzz-1.log)
**アーティファクト**:
- `fuzz/artifacts/fuzz_pipeline/crash-5e57d8223a3950f2b0b75df8b68b38ab9a9c38fd`
- `fuzz/artifacts/fuzz_pipeline/crash-509961e8de1f9df748f5d3c3236d40ab5e74f6db`
**ステータス**: **修正済み** — ブロック引用のネスト深度を最大10段に制限 (`MAX_BLOCKQUOTE_DEPTH`)。11段目以降は `#quote` を出力せず内容のみ出力する (`BlockQuoteCapped`)。テスト `test_blockquote_depth_capped` 追加。

### 再現入力

```markdown
> > > > > > > > > > > > deeply nested blockquote
```

### 修正内容

`markup.rs` で `BlockQuoteCapped` バリアントを追加。ネスト深度が `MAX_BLOCKQUOTE_DEPTH` (10) 以上の場合、
`#quote(block: true)[...]` を出力せずスタックに `BlockQuoteCapped` を積む。
`End(BlockQuote)` 時に `BlockQuoteCapped` なら閉じブラケットを出力しない。

---

## ~~Bug 3: fuzz_convert ソースマップのアサーション失敗~~ (修正済み)

**重要度**: Medium — fuzz_convert ターゲットのみ
**発見**: fuzz_convert
**アーティファクト**: `fuzz/artifacts/fuzz_convert/crash-823d13a04190e19be4673a91b6d19ed8f2e1561a`
**ステータス**: **修正済み** — `Event::Rule` に `block_depth == 0` ガードを追加。テスト `test_rule_inside_list_source_map` 追加。

### 再現入力

```
+	---
```

(unorderedリスト `+` + タブ + 水平線 `---` + タブ2つ)

### 症状

`fuzz_convert` ターゲットのソースマップ検証アサーション (5: typst_byte_range がソート済みで重複なし) が失敗する。

### 原因

`Event::Rule` はリーフイベント（Start/End ペアを持たない）。
修正前は `block_depth` チェックなしに無条件で `BlockMapping` を push していた。

pulldown-cmark は `+\t---` を以下のイベント列に解析する:

```
Start(List(None))   ← + は unordered list marker
  Start(Item)
    Rule            ← --- が thematic break として解析される
  End(Item)
End(List(None))
```

List 内 (`block_depth == 1`) で Rule が発火すると:
1. Rule の `BlockMapping { typst: 2..25, md: 2..5 }` が先に push
2. List の `End` で `BlockMapping { typst: 0..25, md: 0..7 }` が push
3. → **範囲が重複** → fuzz assertion 失敗

### 修正内容

`markup.rs` の `Event::Rule` ハンドラに `block_depth == 0` ガードを追加。
リスト内で Rule が発生してもトップレベルの `BlockMapping` を出力しない。
回帰テスト `test_rule_inside_list_source_map` 追加。

---

## ~~Bug 4: ルーズリスト・SoftBreakでリストアイテムのテキスト折り返しインデントが壊れる~~ (修正済み)

**重要度**: Medium — レンダリング結果のレイアウト崩れ
**発見**: 手動テスト（test-list-wrap2.md）
**ステータス**: **修正済み** — `Start(Paragraph)` と `SoftBreak` のリストアイテム内処理を修正。

### 症状

ルーズリスト（アイテム間に空行があるリスト）で、マーカー `- ` とテキストが分離し、
テキストがリストアイテムの外に出る。また SoftBreak による継続行にインデントがなく、
Typst 的にリストアイテム外のテキストとして扱われる。

### 原因

1. `Event::Start(Tag::Paragraph)` がリストアイテム内でもコンテキストを見ずに `\n\n` を挿入
   → `- \n\nテキスト` となりマーカーとテキストが分離
2. `Event::SoftBreak` がリストアイテム内でもインデントなしの `\n` を出力
   → 継続行がリストアイテム外に出る

### 修正内容

`markup.rs` で2箇所を修正:

1. **`Start(Paragraph)` のリストアイテム内処理**: スタックに `Container::Item` がある場合、
   最初のパラグラフ（マーカー直後）は何も挿入せず、2つ目以降は `\n\n` + インデントを挿入。
2. **`SoftBreak` のリストアイテム内処理**: スタックに `Container::Item` がある場合、
   `\n` の後に `(list_depth - 1) * 2 + 2` スペースのインデントを挿入。
   テーブルセル内（`cell_buf` あり）の場合はインデント不要。

### 追加テスト

- `test_loose_unordered_list` — ルーズ箇条書きでテキストがマーカー直後に付く
- `test_loose_ordered_list` — ルーズ番号リストで同上
- `test_list_item_softbreak` — 継続行がインデント付き
- `test_loose_nested_list` — ネストされたルーズリストの正しいインデント
- `test_table_cell_softbreak` — 非リスト SoftBreak でインデントが入らない

---

## Bug 5: mermaid-rs-renderer が不正な SVG を出力する (ワークアラウンド済み)

**重要度**: Medium — SVG が Typst の XML パーサーで拒否される
**発見**: Mermaid 対応実装時
**ステータス**: **ワークアラウンド済み** — `diagram.rs` の `fix_svg_font_family()` で修正

### 症状

`mermaid-rs-renderer::render()` が生成する SVG の `font-family` 属性に
エスケープされていないダブルクォートが含まれる:

```xml
font-family="Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif"
```

`"Segoe UI"` の内側のダブルクォートが XML 属性値を途中で閉じてしまい、
Typst の SVG パーサー (usvg) が `expected a whitespace not 'S' at 1:1400` で失敗する。

### 原因

mermaid-rs-renderer v0.2.0 のバグ。`font-family` CSS 値をそのまま
XML 属性に埋め込んでおり、CSS では合法な `"Segoe UI"` が XML では不正。
正しくは `&quot;` にエスケープするか、シングルクォートを使うべき。

### ワークアラウンド

`diagram.rs` の `fix_svg_font_family()` が SVG 文字列を後処理し、
`font-family` 属性内の内側ダブルクォートをシングルクォートに置換する。
上流への報告・修正後にワークアラウンドを除去できる。

---

## Bug 6: Typst raw 要素の ShowSet が暗黙の 0.8em スケーリングを適用する (ワークアラウンド済み)

**重要度**: Low — テーマ作成時に知っておくべき仕様
**発見**: インラインコードのフォントサイズを相対値に変更する際
**ステータス**: **ワークアラウンド済み** — テーマファイルで `/ 0.8` 補正を使用

### 症状

テーマの show rule でインラインコードのフォントサイズを `em` 単位で指定すると、
意図した比率より小さくレンダリングされる。例: `size: 0.85em` で期待値 10.2pt (12pt×0.85)
のところ 8.2pt (12pt×0.85×0.8) になる。絶対値 (`10pt`) では正しく動作する。

### 原因

Typst の `RawElem` は `ShowSet` トレイト実装で組み込みの set rule を持つ
(`typst-library 0.14.2`, `src/text/raw.rs` L508):

```rust
out.set(TextElem::size, TextSize(Em::new(0.8).into()));
```

この `0.8em` はユーザーの show rule が発火する**前に**適用される。そのため
show rule 内での `em` コンテキストは `0.8 × 親サイズ` であり、
`size: Xem` と書くと実効値は `X × 0.8 × 親サイズ` になる。

monospace フォントは視覚的に大きく見えるため 0.8 倍にしているとの
ドキュメントコメントあり (`raw.rs` L90-114):

> By default, the `raw` element uses the `DejaVu Sans Mono` font (included
> with Typst), with a smaller font size of `{0.8em}` (that is, 80% of
> the global font size). This is because monospace fonts tend to be visually
> larger than non-monospace fonts.

### ワークアラウンド

Typst 公式ドキュメントの推奨パターン `/ 0.8` を使用して補正する:

```typst
// 実効 0.85 × 親サイズ
text(size: 0.85em / 0.8, it)

// Typst 公式例: 完全にリセット
#show raw.where(block: true): set text(1em / 0.8)
```

テーマファイルでは `size`, `inset`, `outset` すべてに `/ 0.8` を付与している。

---

## ~~Bug 7: インラインコードの背景が次の行に重なる~~ (修正済み)

**重要度**: Low — レンダリングの見た目の問題
**発見**: README.md のビューア表示で目視確認
**ステータス**: **修正済み** — テーマの `inset` Y成分を `outset` に変更

### 症状

インラインコードを含む段落で、背景矩形が次の視覚行のテキストと重なる。段落が折り返されてインラインコードが行末付近にある場合に顕著。

### 原因

Typst の `box` における `inset` は CSS の `padding` 相当で、フレームサイズを拡大する。`inset: (y: 2pt)` で box が上下計 4pt 高くなり、行間 (`leading`) は固定のため背景矩形の下端が次の行に食い込む。

### 修正内容

`inset` の Y 成分を `outset` に移動。`outset` は背景の描画位置のみ拡張し、レイアウト上のサイズは変えない。

```typst
// Before: box(fill: ..., inset: (x: 4pt, y: 2pt), radius: 3pt, ...)
// After:  box(fill: ..., inset: (x: 4pt), outset: (y: 2pt), radius: 3pt, ...)
```

対象: `themes/catppuccin.typ`, `themes/catppuccin-latte.typ`。回帰テスト `test_inline_code_no_line_overlap` 追加。

---

## 備考

- Bug 1, 2, 3, 4 はすべて修正済み。
- fuzz_pipeline ターゲットは `compile_document` のエラーを `panic!` で処理している
  (`fuzz_pipeline.rs:25`)。未知の入力で Typst コンパイルエラーが
  起きうるため、`panic!` → `return` への変更も検討すべき。
- `fuzz-2.log` ではクラッシュなし。
- 残りのアーティファクト (`crash-b305e5d4958745f890f1f79742e6651461a6ae15`,
  `crash-e4ff42b87d848fb8f077d67de5c367c89057e3de`) は過去のfuzz runからのもので、
  上記バグの類似ケース。
