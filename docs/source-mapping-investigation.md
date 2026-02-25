# ソースマッピング調査: Visual Line → Markdown ソース行の逆引き

## 概要

TUI ビューアでのテキストコピー（ヤンク）機能を設計するにあたり、
レンダリング済みフレームツリーからテキストを再結合する方式と、
オリジナル Markdown ソースへの逆マッピング方式を比較調査した。

**結論**: Glyph::span → Source::range() → convert.rs SourceMap のチェーンで、
visual line からオリジナル Markdown ソース行への逆引きが実現可能。
ヤンク出力を常に安定した Markdown 断片にできる。

---

## 動機: テキスト再結合の不安定性

画像ベースの TUI ビューアでは、ターミナルの通常テキスト選択が使えない。
ヤンク機能の実装には、visual line のテキスト内容を何らかの方法で取得する必要がある。

### 方式 A: フレームツリーからのテキスト再結合

TextItem::text を Y 座標でグループ化し、X 座標順に結合してプレーンテキストを得る方式。

**不安定要因:**

1. **CJK テキスト分割**: Typst のシェイパーはフォント境界でテキストランを分割する。
   同一行の "Rust" + "における" + "エラーハンドリング" が3つの TextItem になる
   （`paged-document-structure.md` L216-218 参照）

2. **インラインコードの構造**: `Result<T, E>` のようなインラインコードは
   `Group(Shape + Text)` の入れ子構造。背景 Shape と Text のペアを正しく処理する必要がある

3. **段落折り返し**: 1つの Markdown 段落が複数の top-level Group に分かれる
   （`paged-document-structure.md` L196-211 参照）

4. **テーブルセル**: 複雑なネスト構造。ヘッダは Tag + Text、データ行は Group 内に
   さらに Group がネスト

5. **インライン装飾の消失**: 再結合テキストからは太字・斜体・リンク URL 等の情報が失われる。
   `**bold**` → "bold"、`[text](url)` → "text" になってしまう

6. **Markdown としての再利用不可**: 再結合テキストはプレーンテキストであり、
   ペースト先で Markdown として再利用できない

### 方式 B: オリジナル Markdown ソースへの逆マッピング

visual line → Markdown ソース行の対応を追跡し、
ヤンク時にオリジナル Markdown ソースからそのまま切り出す方式。

**利点:**
- 出力は常にオリジナル Markdown の断片（安定性保証）
- インライン装飾・リンク URL・コードフェンス等がすべて保存される
- ペースト先で Markdown として再利用可能

---

## パイプラインの情報フロー分析

各段階で利用可能な情報と損失ポイントを記録する。

### 段階 1: Markdown ソース → pulldown-cmark イベント

```
Markdown ソース（行番号付き）
  ↓ pulldown-cmark::Parser::new_ext(markdown, options)
  ↓ .into_offset_iter()   ★ 未使用だが利用可能
(Event, Range<usize>) ストリーム
```

- `into_offset_iter()` は各イベントに Markdown ソース内のバイト範囲を付与する
- `Range<usize>` は Markdown ソーステキスト内のバイトオフセット
- **現状**: `convert.rs` は `Parser::new_ext()` のみ使用し、オフセット情報を捨てている（L26）

### 段階 2: pulldown-cmark イベント → Typst マークアップ

```
(Event, Range<usize>) ストリーム
  ↓ convert.rs: markdown_to_typst()
Typst マークアップ文字列 (content_text)
```

- `convert.rs` はイベントを順次処理し、Typst マークアップ文字列を構築する
- **現状**: Markdown バイト範囲も Typst 出力内の位置も追跡していない
- **可能**: イベント処理中に `(typst_byte_range, md_byte_range)` のペアを記録できる

### 段階 3: Typst マークアップ → main.typ ソース

```
content_text (Typst マークアップ)
  ↓ MluxWorld::new(theme_text, content_text, width)
main.typ = "{theme_text}\n#set page(width: {width}pt)\n{content_text}\n"
```

- `world.rs` L29-31: テーマ + 幅設定 + コンテンツを単一ファイルに結合
- content_text の開始オフセット（prefix_len）は確定的に計算可能:
  `prefix_len = theme_text.len() + 1 + format!("#set page(width: {width}pt)\n").len()`
- **単一ファイル**: `FileId::new(None, VirtualPath::new("main.typ"))` のみ

### 段階 4: main.typ → PagedDocument（コンパイル）

```
main.typ (Source)
  ↓ typst::compile::<PagedDocument>(&world)
PagedDocument
  └── pages[0].frame
        └── items: Vec<(Point, FrameItem)>
              └── FrameItem::Text(TextItem)
                    └── glyphs: Vec<Glyph>
                          └── span: (Span, u16)  ★ ソース位置情報
```

- Typst コンパイラは各グリフにソース位置（Span）を割り当てる
- `Glyph::span` は `(Span, u16)` — Span が main.typ 内の位置を指す
- `paged-document-structure.md` L124-132 参照

### 段階 5: PagedDocument → VisualLine

```
PagedDocument
  ↓ extract_visual_lines()
Vec<VisualLine> { y_pt, y_px }
```

- **現状**: Y 座標のみ抽出、テキスト内容もソース位置も捨てている
- **可能**: Glyph::span を解決してソース位置情報を VisualLine に付加できる

### 情報フロー図（損失ポイント付き）

```
Markdown source (line numbers)
  │
  ├─ pulldown-cmark: into_offset_iter()
  │  → (Event, Range<usize>)           ★ MD バイト範囲あり
  │
  ✗ LOST: convert.rs がオフセットを捨てている
  │
  ↓
Typst markup string (content_text)
  │
  ├─ MluxWorld: prefix_len は確定的に計算可能
  │
  ↓
main.typ source
  │
  ├─ typst::compile → Glyph::span     ★ main.typ 内バイト位置あり
  │
  ✗ LOST: extract_visual_lines が span を捨てている
  │
  ↓
VisualLine[] (Y coordinate only)
```

2箇所の情報損失を修復すれば、完全なマッピングチェーンが成立する。

---

## Typst Span API の確認結果

typst-syntax 0.14.2 の公開 API を cargo doc で確認した。

### Span 構造体

```rust
pub struct Span(NonZeroU64);
```

ファイル内の範囲を定義する。コンパイラ全体でソース位置追跡に使用される。

**主要メソッド:**

| メソッド | シグネチャ | 用途 |
|---|---|---|
| `id()` | `fn id(self) -> Option<FileId>` | Span が指すファイルの ID。detached なら None |
| `range()` | `fn range(self) -> Option<Range<usize>>` | raw range span の場合のバイト範囲 |
| `is_detached()` | `fn is_detached(self) -> bool` | ソース位置を持たない span か |

**Span のドキュメントより:**
> - The `.id()` function can be used to get the `FileId` for the span
> - The `WorldExt::range` function can be used to map the span to a `Range<usize>`

### Source 構造体のメソッド

```rust
// Source::range — Span からバイト範囲を取得
pub fn range(&self, span: Span) -> Option<Range<usize>>
// "Get the byte range for the given span in this file."

// Source::find — Span から構文ノードを取得
pub fn find(&self, span: Span) -> Option<LinkedNode<'_>>
// "Find the node with the given span."
```

**`Source::range(span)` が本調査の核心。**
Glyph::span.0 を渡すと、main.typ 内のバイト範囲 `Range<usize>` が返る。

### Glyph::span の構造

```rust
pub struct Glyph {
    pub id: u16,
    pub x_advance: Em,
    pub x_offset: Em,
    pub y_advance: Em,
    pub y_offset: Em,
    pub range: Range<u16>,     // TextItem.text 内のバイト範囲
    pub span: (Span, u16),     // ソースコード上の位置
}
```

`span` フィールドの `(Span, u16)`:
- `Span`: main.typ 内のソース位置（テキストノード全体の範囲）
- `u16`: ノード内のオフセット（個別文字のより精密な位置特定用）

ブロック単位のマッピングには `Span` 部分（`.0`）だけで十分。

---

## マッピングチェーンの詳細設計

### 完全なチェーン

```
Glyph::span.0                          // (Span)
  → Source::range(span)                 // Option<Range<usize>> in main.typ
  → subtract prefix_len                // offset in content_text
  → SourceMap binary search            // BlockMapping を検索
  → BlockMapping.md_byte_range         // Range<usize> in Markdown source
  → byte offset → line number          // MD 行番号に変換
```

### Layer 1: convert.rs の SourceMap

`markdown_to_typst()` を `into_offset_iter()` に切り替え、
各ブロックイベントで Typst 出力バイト範囲と MD バイト範囲のペアを記録する。

```
struct BlockMapping {
    typst_byte_range: Range<usize>,  // content_text 内のバイト範囲
    md_byte_range: Range<usize>,     // Markdown ソース内のバイト範囲
}

struct SourceMap {
    blocks: Vec<BlockMapping>,       // typst_byte_range.start でソート済み
}
```

pulldown-cmark のブロックイベントと対応:

| pulldown-cmark イベント | 記録タイミング |
|---|---|
| `Start(Heading)` .. `End(Heading)` | 見出し 1 ブロック |
| `Start(Paragraph)` .. `End(Paragraph)` | 段落 1 ブロック |
| `Start(CodeBlock)` .. `End(CodeBlock)` | コードブロック 1 ブロック |
| `Start(List)` .. `End(List)` | リスト全体 1 ブロック |
| `Start(BlockQuote)` .. `End(BlockQuote)` | 引用全体 1 ブロック |
| `Start(Table)` .. `End(Table)` | テーブル全体 1 ブロック |
| `Rule` | 水平線 1 ブロック |

### Layer 2: MluxWorld の prefix_len

`MluxWorld::new()` で content_text の開始位置を記録する。

```rust
// world.rs L29-31 の format! を分解:
let prefix = format!("{theme_text}\n#set page(width: {width}pt)\n");
let prefix_len = prefix.len();
// main.typ 内の byte_offset から content_text 内の offset を得る:
// content_offset = main_typ_byte_offset - prefix_len
```

### Layer 3: extract_visual_lines での Span 解決

現在の `extract_visual_lines(document, ppi)` に `Source` と `SourceMap` を追加して渡す。

各 visual line の TextItem から Glyph::span を取得 → Source::range() で
main.typ 内バイト範囲 → prefix_len を引いて content_text 内オフセット →
SourceMap の blocks を二分探索 → 該当 BlockMapping の md_byte_range →
MD ソース内のバイト範囲から行番号を算出。

### Layer 4: VisualLine に MD 行範囲を付加

```
struct VisualLine {
    y_pt: f64,
    y_px: u32,
    md_line_range: Option<(usize, usize)>,  // (start, end) 1-based inclusive
}
```

`None` になるケース: テーマ由来のテキスト、detached Span、prefix 内のテキスト。
ヤンク時に `None` の行はスキップする。

---

## フレームツリーのブロック構造と Span の対応

`paged-document-structure.md` の実機ダンプに基づき、各ブロック要素の
フレームツリー構造と、Span がどこを指すかを記録する。

### 見出し

```
Group (40.0, 40.0)pt
  Text "Rust"              ← Glyph::span → "= Rust..." の中の "Rust" 部分
  Text "における"           ← Glyph::span → 同上ブロック内
  Text "エラーハンドリング"  ← Glyph::span → 同上ブロック内
```

同一見出しブロック内の全 TextItem の Span は、main.typ 内の同一見出し行を指す。
`Source::range()` の結果は同一 `BlockMapping.typst_byte_range` 内に収まる。

### 段落（折り返しあり）

```
Group (40.0, 103.3)pt     ← 1行目 (visual line N)
  Text "Rust"
  Text "の"
  Text "エラーハンドリング..."
Group (40.0, 126.9)pt     ← 2行目 (visual line N+1) ← 折り返し
  Text "コンパイル..."
```

折り返しで複数 Group に分かれるが、全 TextItem の Span は
同一段落ブロックの Typst ソース範囲を指す。
→ 複数 visual line が同一 `BlockMapping` にマッピングされる。
→ ヤンク時は段落全体の MD ソースが切り出される（正しい挙動）。

### コードブロック

```
Group (40.0, 228.3)pt     ← コードブロック全体
  Shape (背景矩形)
  Group (52.0, 240.3)pt   ← コード本体
    Group → Text "fn read_config(...)"   ← 1行目
    Group → Text "let content = ..."     ← 2行目
    Group → Text "let config = ..."      ← 3行目
    Group → Text "Ok(config)"            ← 4行目
    Text "}"                             ← 5行目
```

コードブロック内の各行は個別の Group/Text。
各行の Glyph::span は Typst ソース内のコードブロック範囲を指すが、
行ごとに異なるバイトオフセットを持つ。

**コードブロックの行単位マッピングの可能性:**
Typst ソース内でコードブロックの内容は改行区切りで保存されるため、
Span のバイト範囲から特定のコード行を識別できる可能性がある。
ただし、convert.rs の SourceMap が行単位の粒度を持つ必要がある。

### テーブル

```
Group (40.0, 362.7)pt     ← テーブル全体
  Shape (×12, 罫線・背景)
  Tag → Text "クレート"    ← ヘッダセル
  Tag → Text "特徴"
  Tag → Text "用途"
  Group (データ行1)
  Group (データ行2)
  Group (データ行3)
```

テーブルは convert.rs で `#table(columns: N, ...)` に変換される。
Glyph::span はこの `#table(...)` 式内を指す。
テーブル全体が1つの BlockMapping にマッピングされる。

### リスト

```
Group (40.0, 626.0)pt     ← リスト全体
  Text "1."               ← マーカー
  Group → テキスト          ← 項目1
  Text "2."
  Group → テキスト          ← 項目2
  ...
```

リスト全体が1つの Group。convert.rs では各項目が `- ` or `+ ` で始まる行として出力。
Glyph::span はリスト全体の Typst ソース範囲内を指す。

### 引用ブロック

```
Group (40.0, 499.3)pt     ← 引用全体
  Shape (左ボーダー)
  Group → Text (引用テキスト)
  Group → Group (ネスト引用)
```

convert.rs で `#quote(block: true)[...]` に変換。
全体が1つの BlockMapping。

---

## MluxWorld のソースファイル構造

`world.rs` L27-33 より:

```rust
pub fn new(theme_text: &str, content_text: &str, width: f64) -> Self {
    let main_text = format!(
        "{theme_text}\n#set page(width: {width}pt)\n{content_text}\n"
    );
    Self::from_source(&main_text, true)
}
```

main.typ の構造:

```
[0..theme_len]                    ← テーマ (catppuccin.typ の内容)
[theme_len..theme_len+1]          ← "\n"
[theme_len+1..prefix_len]         ← "#set page(width: {width}pt)\n"
[prefix_len..prefix_len+content_len]  ← content_text (convert.rs の出力)
[prefix_len+content_len..]        ← "\n"
```

`prefix_len` は `theme_text.len() + 1 + format!("#set page(width: {width}pt)\n").len()`。

main.typ 内のバイトオフセット `b` が content_text 内のオフセット `c` に対応:
`c = b - prefix_len` （`c >= 0` の場合のみ有効）

**注意**: テーマファイルの長さが変わると prefix_len も変わる。
prefix_len はコンパイル時に確定的に計算するため、テーマ変更にも対応する。

単一ファイル（`FileId::new(None, VirtualPath::new("main.typ"))`）のみの
仮想ファイルシステムなので、全 Span は同一ファイルを指す。

---

## pulldown-cmark のオフセット情報

### into_offset_iter() API

```rust
let parser = Parser::new_ext(markdown, options);
for (event, range) in parser.into_offset_iter() {
    // range: Range<usize> — Markdown ソース内のバイト範囲
    // event: Event — パーサイベント
}
```

- `Range<usize>` は Markdown ソーステキストのバイトオフセット
- ブロック開始・終了イベントにも範囲が付く
- テキストイベントの範囲は、そのテキスト片の Markdown ソース内の位置

### イベントと範囲の例

入力 Markdown:
```markdown
# Heading

Paragraph with **bold** text.
```

イベントストリーム:
```
Start(Heading{level:1})  range: 0..11    "# Heading\n"
Text("Heading")          range: 2..9     "Heading"
End(Heading(1))          range: 0..11

Start(Paragraph)         range: 12..42   "Paragraph with **bold** text.\n"
Text("Paragraph with ")  range: 12..27
Start(Strong)            range: 27..35   "**bold**"
Text("bold")             range: 29..33
End(Strong)              range: 27..35
Text(" text.")           range: 35..41
End(Paragraph)           range: 12..42
```

ブロックイベントの `range` がブロック全体の MD バイト範囲を与える。

---

## convert.rs の現状と変更方針

### 現状 (convert.rs L22-37)

```rust
pub fn markdown_to_typst(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(markdown, options);
    // ...
    for event in parser {
        // event のみ、Range なし
    }
}
```

### 変更方針

1. `Parser::new_ext()` → `.into_offset_iter()` に切り替え
2. `for event in parser` → `for (event, range) in parser.into_offset_iter()` に変更
3. ブロック開始時に Typst 出力の現在位置と MD range を記録
4. ブロック終了時に BlockMapping を確定
5. 戻り値を `(String, SourceMap)` に拡張

### 追跡が必要なブロックイベント

```
Start(Heading)     → md_range 記録開始、typst_start = output.len()
End(Heading)       → BlockMapping { typst: typst_start..output.len(), md: range }

Start(Paragraph)   → 同上
End(Paragraph)     → 同上

Start(CodeBlock)   → 同上
End(CodeBlock)     → 同上

Start(List)        → 同上
End(List)          → 同上

Start(BlockQuote)  → 同上
End(BlockQuote)    → 同上

Start(Table)       → 同上（テーブルは End 時にまとめて emit するため、End 時の位置が正しい）
End(Table)         → 同上

Rule               → 単独イベント、typst 出力位置と md range を即座に記録
```

**ネスト注意**: BlockQuote 内の Paragraph 等、ネストしたブロックは
最外のブロック（BlockQuote）の BlockMapping に含まれる。
SourceMap はトップレベルブロックのみ記録する方針が最もシンプル。

---

## VisualLine → BlockMapping の検索アルゴリズム

### 手順

1. visual line の TextItem からグリフを1つ取得（最初の TextItem の最初の Glyph で十分）
2. `Glyph::span.0` で Span を取得
3. `source.range(span)` で main.typ 内のバイト範囲 `Range<usize>` を取得
4. `range.start - prefix_len` で content_text 内のオフセットを計算
5. SourceMap.blocks を二分探索:
   `blocks.binary_search_by(|b| b.typst_byte_range.start.cmp(&content_offset))`
6. 該当する BlockMapping の `md_byte_range` を取得
7. MD バイト範囲を行番号に変換

### 行番号変換

Markdown ソーステキストに対して、事前にバイトオフセット→行番号の
テーブルを構築する（改行位置のプレフィックスサム）。

```
fn byte_offset_to_line(source: &str, offset: usize) -> usize {
    source[..offset].bytes().filter(|&b| b == b'\n').count() + 1
}
```

ブロックの開始行と終了行:
```
start_line = byte_offset_to_line(md_source, md_byte_range.start)
end_line = byte_offset_to_line(md_source, md_byte_range.end.saturating_sub(1))
```

---

## ヤンク時の挙動

### ブロック単位のヤンク

ユーザーが visual line N..M を選択してヤンク（`y`）した場合:

1. 各 visual line の `md_line_range` を収集
2. 全範囲の union を取る: `min(start) .. max(end)`
3. オリジナル Markdown ソースの該当行をそのまま切り出す
4. OSC 52 でクリップボードに送信

### 出力例

Markdown ソース:
```markdown
## 基本パターン

Rustのエラーハンドリングには
複数のパターンがあります。

```rust
fn example() -> Result<(), Error> {
    let data = read()?;
    Ok(())
}
```⁠
```

ユーザーが「Rustのエラーハンドリングには」の visual line を含む範囲を選択した場合:
→ 段落全体（`Rustのエラーハンドリングには\n複数のパターンがあります。`）が切り出される。

**ヤンク出力はオリジナル Markdown そのもの** — 太字マーク、リンク URL、
コードフェンスすべてが保存される。

### 粒度の検討

| 要素 | ヤンク粒度 | 理由 |
|---|---|---|
| 見出し | 行単位（= ブロック単位） | 見出しは常に 1 visual line |
| 段落 | ブロック単位 | 折り返し途中の行だけ切り出しても無意味 |
| コードブロック | ブロック単位 | フェンス（` ``` `）を含む全体を保持 |
| リスト | ブロック単位 | リスト全体の構造を保持 |
| テーブル | ブロック単位 | テーブル全体の構造を保持 |
| 引用 | ブロック単位 | 引用マーカーを含む全体を保持 |
| 水平線 | ブロック単位 | `---` 1行 |

コードブロック内の行単位ヤンクは将来の拡張として検討可能。
SourceMap の粒度をコードブロック内の行レベルまで細かくすることで対応できるが、
初期実装ではブロック単位で十分。

---

## エッジケースと制約

### Span 解決が失敗するケース

1. **detached Span**: テーマ由来のテキスト（行番号等のスタイル設定）は
   detached Span を持つ可能性がある。`span.is_detached()` でチェックし、
   `md_line_range = None` とする

2. **prefix 範囲内の Span**: テーマや `#set page(width:...)` 由来のテキストは
   content_text の範囲外。`content_offset < 0` でフィルタ

3. **Source::range() が None を返す**: Span の種類によっては解決不能。
   同上の対応

### テーマ変更の影響

prefix_len はテーマテキストの長さに依存する。
テーマを変更すると prefix_len が変わるが、MluxWorld の構築時に毎回計算するため問題ない。

### ドキュメント再コンパイル時

ターミナルリサイズ時にドキュメントを再コンパイルする（viewer.rs の outer loop）。
再コンパイル時に SourceMap は不変（Markdown ソースが変わらない限り）、
prefix_len は width 変更で変わる可能性がある → 再計算が必要。
extract_visual_lines も再実行されるため、自動的に正しいマッピングが得られる。

### テーブルの emit タイミング

convert.rs ではテーブルは `End(Table)` 時にまとめて emit される（L198-206）。
`Start(Table)` の range はテーブル全体の MD 範囲を持つが、
Typst 出力は `End(Table)` 後に書き込まれる。
→ typst_byte_range の記録は `End(Table)` 後の output.len() を使う。

---

## 必要な API 変更のまとめ

### convert.rs

- `markdown_to_typst(markdown: &str) -> String`
  → `markdown_to_typst(markdown: &str) -> (String, SourceMap)`
- `Parser::new_ext()` → `.into_offset_iter()`
- 新規: `SourceMap` 構造体と `BlockMapping` 構造体

### world.rs

- `MluxWorld` に `content_offset: usize` フィールド追加
  （main.typ 内で content_text が始まるバイトオフセット）
- `MluxWorld::source()` の戻り値 `Source` へのアクセサ追加
  （Span 解決に `Source::range(span)` が必要）

### strip.rs

- `VisualLine` に `md_line_range: Option<(usize, usize)>` フィールド追加
- `extract_visual_lines()` のシグネチャ拡張:
  `Source`, `content_offset`, `SourceMap`, Markdown ソース文字列を受け取る
- Glyph::span 解決ロジックの追加

### viewer.rs

- `build_strip_document()` で SourceMap を受け渡し
- ヤンク時に `md_line_range` を使って MD ソースから行を切り出し
- オリジナル Markdown ソーステキストをビューアスコープに保持
