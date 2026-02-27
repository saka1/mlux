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

## Bug 2: 深いブロック引用ネストでTypst show ruleの深度上限超過

**重要度**: High — コンパイルが失敗する
**発見**: fuzz_pipeline (fuzz-0.log, fuzz-1.log)
**アーティファクト**:
- `fuzz/artifacts/fuzz_pipeline/crash-5e57d8223a3950f2b0b75df8b68b38ab9a9c38fd`
- `fuzz/artifacts/fuzz_pipeline/crash-509961e8de1f9df748f5d3c3236d40ab5e74f6db`

### 再現入力

```markdown
> > > > > > > > > > > > deeply nested blockquote
```

または大量の `>` を含む壊れたMarkdown。

### 症状

Typstコンパイル時に `error: maximum show rule depth exceeded` で失敗。

### 原因

`convert.rs:127-139` で `>` のネストごとに `#quote(block: true)[...]` を入れ子にする。
テーマ (`themes/catppuccin.typ:24-27`) の show rule が再帰的にトリガーされ、
Typstの show rule 深度上限を超える。

```rust
// convert.rs:127-139 — ネスト制限なし
Event::Start(Tag::BlockQuote(_)) => {
    output.push_str("#quote(block: true)[");
    stack.push(Container::BlockQuote);
}
```

```typst
// themes/catppuccin.typ:24-27 — show rule が再帰的にトリガー
#show quote.where(block: true): it => block(
  inset: (left: 16pt, y: 8pt),
  stroke: (left: 3pt + rgb("#89b4fa")),
  text(fill: rgb("#a6adc8"), it.body))
```

### Typstエラー出力

```
error: maximum show rule depth exceeded
  --> main.typ:137:2
   137 | #quote(block: true)[
  hint: maybe a show rule matches its own output
  hint: maybe there are too deeply nested elements
```

---

## Bug 3: fuzz_convert ソースマップのアサーション失敗

**重要度**: Medium — fuzz_convert ターゲットのみ
**発見**: fuzz_convert
**アーティファクト**: `fuzz/artifacts/fuzz_convert/crash-823d13a04190e19be4673a91b6d19ed8f2e1561a`

### 再現入力

```
+	---
```

(orderedリスト `+` + タブ + 水平線 `---` + タブ2つ)

### 症状

`fuzz_convert` ターゲットのソースマップ検証アサーションのいずれかが失敗する。
具体的にどのアサーションかは未調査。

```rust
// fuzz_convert.rs で検証しているアサーション:
// 1. typst_byte_range.end <= with_map.len()
// 2. md_byte_range.end <= markdown.len()
// 3. typst_byte_range が反転していない
// 4. md_byte_range が反転していない
// 5. typst_byte_range がソート済みで重複なし
```

### 原因

未調査。orderedリスト開始 (`+`) と水平線 (`---`) の組み合わせで
pulldown-cmarkのイベント列が予期しないパターンになり、
ソースマップの `block_starts` / `block_depth` 追跡がずれる可能性。

---

## 備考

- fuzz_pipeline ターゲットは `compile_document` のエラーを `panic!` で処理している
  (`fuzz_pipeline.rs:25`)。Bug 1, 2 の修正後も、未知の入力で Typst コンパイルエラーが
  起きうるため、`panic!` → `return` への変更も検討すべき。
- `fuzz-2.log` ではクラッシュなし。
- 残りのアーティファクト (`crash-b305e5d4958745f890f1f79742e6651461a6ae15`,
  `crash-e4ff42b87d848fb8f077d67de5c367c89057e3de`) は過去のfuzz runからのもので、
  上記バグの類似ケース。
