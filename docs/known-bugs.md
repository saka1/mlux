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

`convert.rs` の `Event::Start(Tag::Link)` / `Event::End(TagEnd::Link)` ハンドラで
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

`convert.rs` で `BlockQuoteCapped` バリアントを追加。ネスト深度が `MAX_BLOCKQUOTE_DEPTH` (10) 以上の場合、
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

`convert.rs` の `Event::Rule` ハンドラに `block_depth == 0` ガードを追加。
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

`convert.rs` で2箇所を修正:

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

## 備考

- Bug 1, 2, 3, 4 はすべて修正済み。
- fuzz_pipeline ターゲットは `compile_document` のエラーを `panic!` で処理している
  (`fuzz_pipeline.rs:25`)。未知の入力で Typst コンパイルエラーが
  起きうるため、`panic!` → `return` への変更も検討すべき。
- `fuzz-2.log` ではクラッシュなし。
- 残りのアーティファクト (`crash-b305e5d4958745f890f1f79742e6651461a6ae15`,
  `crash-e4ff42b87d848fb8f077d67de5c367c89057e3de`) は過去のfuzz runからのもので、
  上記バグの類似ケース。
