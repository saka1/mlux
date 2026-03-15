# ContentIndex: Markdown ↔ Frame tree 双方向インデックスの設計

## 解きたい問題

mlux のビューアは 2 つの座標空間を橋渡しする機能を持つ:

| 機能 | 方向 | 入力 | 出力 |
|------|------|------|------|
| Yank | 描画 → ソース | Frame tree の Span（ビジュアル行） | Markdown の行テキスト |
| Highlight | ソース → 描画 | Markdown 上の regex マッチ | Frame tree 上のピクセル矩形 |

**本質的には (A) Markdown ソース ↔ (C) Frame tree のマッピングを行うインデックスが必要。**

Markdown → Typst 変換 → コンパイル → レンダリングという多段パイプラインを経るため、
この対応付けは一般に難しい。テキストのエスケープ、数式の LaTeX→Typst 変換、
Mermaid の SVG 化、テーブル構造の再編成など、変換ごとに情報が失われたり変形したりする。

**この難しさは ContentIndex というデータ構造に閉じ込める。**
ContentIndex の外側からは「Markdown 位置 → Frame 位置」「Frame 位置 → Markdown 位置」
という単純な操作だけが見える。エスケープ補正もニューライン数ヒューリスティックも
TextItem テキスト再検索も、すべて ContentIndex の内部実装として吸収する。

---

## 現状の問題

現在、(A) ↔ (C) のマッピングは 2 つのアドホックな手法で行われている:

**Yank (C→A):**
Span → `Source::range()` → Typst バイト範囲 → `SourceMap` でブロック特定 →
ニューライン数ヒューリスティックで行推定。ヒューリスティックが失敗する場合
（段落の 2 番目以降、テーブル、ネスト blockquote）はブロック全体にフォールバック。

**Highlight (A→C):**
SourceMap を**完全にバイパス**し、Frame tree の TextItem.text に対して regex を再実行。
テキスト内容が Markdown と一致する場合にのみ動作。数式・Mermaid では原理的に破綻。
TextItem 境界をまたぐマッチも不可能。

**共通の問題:**
- 変換の困難さが tile.rs、highlight.rs、mode_search.rs に散在している
- 新しいユースケースごとにアドホックなロジックが増える
- テスト・デバッグが困難（問題箇所の特定に各モジュールの理解が必要）

---

## 座標空間

```
(A) Markdown source      ユーザが書いた原文。バイトオフセット。
                          ── これが「真実」──

(B) Typst content_text   markdown_to_typst() が生成した中間テキスト。
                          エスケープ、マークアップ変換済み。
                          ── ContentIndex の内部にのみ存在する ──

(C) Frame tree            Typst がコンパイル・レンダリングした結果。
                          各グリフに Span (= main.typ バイト位置) が付与。
                          ── ビューアが表示するもの ──
```

(B) は (A) と (C) を繋ぐ中間表現に過ぎない。ContentIndex の公開 API は
**(A) の Markdown 位置** と **(C) の Frame tree Span** だけで語る。
(B) の存在は ContentIndex の内部実装に隠蔽する。

---

## 設計

### 二層構造: ContentIndex と BoundIndex

ContentIndex は `markdown_to_typst()` 時に構築される。
この時点では Typst のコンパイル結果（Source, content_offset）は未知。

コンパイル後、Source と content_offset を束縛して `BoundIndex` を生成する。
**BoundIndex が (A) ↔ (C) の公開 API を提供する。**

```
markdown_to_typst()                compile()
       │                               │
       ▼                               ▼
  ContentIndex  ──── + Source ────► BoundIndex
  (A)↔(B) 内部       + content_offset   (A)↔(C) 公開API
```

### 内部データ: ContentIndex

```rust
/// Markdown ↔ Typst content_text の内部マッピング。
///
/// markdown_to_typst() の変換時に構築される。
/// 直接使用せず、BoundIndex 経由でアクセスする。
#[derive(Debug, Clone)]
pub struct ContentIndex {
    /// テキストスパンマッピング。typst_range.start 昇順でソート。
    spans: Vec<TextSpan>,

    /// 逆引き用: md_range.start 昇順の spans へのインデックス。
    md_order: Vec<usize>,

    /// ブロック単位マッピング。typst_range.start 昇順。
    /// yank_lines（ブロック全体の yank）や mermaid/画像のブロック対応に使用。
    blocks: Vec<BlockSpan>,
}
```

### TextSpan: テキスト単位のマッピング

```rust
/// Markdown と Typst content_text の間のテキストスパン対応。
#[derive(Debug, Clone)]
pub struct TextSpan {
    /// Markdown ソース内のバイト範囲。
    pub md_range: Range<usize>,
    /// Typst content_text 内のバイト範囲。
    pub typst_range: Range<usize>,
    /// テキストの種別。変換の性質を決定する。
    pub kind: SpanKind,
}
```

### SpanKind: 変換の性質の分類

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// 平文テキスト。Typst 側はエスケープ済み (`#` → `\#` 等) だが、
    /// レンダリング結果のテキスト内容は Markdown と一致する。
    /// スパン内の文字単位オフセット変換が可能（エスケープ走査で補正）。
    Plain,

    /// コードブロック・インラインコード内のテキスト。
    /// 内容が完全に一致する（エスケープなし）。
    /// スパン内の文字単位オフセット変換が正確（1:1 対応）。
    Code,

    /// 数式（インライン/ディスプレイ）。
    /// LaTeX → mitex → Typst math に変換済み。テキスト内容は不一致。
    /// 範囲全体としてのマッピングのみ。スパン内部の位置推定は不可。
    Math,

    /// SoftBreak / HardBreak。
    /// Markdown の改行位置と Typst の改行/インデントの対応。
    Break,

    /// 画像・Mermaid ダイアグラム。
    /// Typst 側は `#image("...")` 呼び出し。Frame tree 上は ImageItem。
    /// テキストとしてのマッチング対象外。ブロックレベルの位置対応のみ。
    Opaque,
}
```

### BlockSpan: ブロック単位のマッピング

```rust
/// ブロック単位のマッピング（段落、見出し、コードブロック、リスト、テーブル等）。
///
/// 用途:
/// - yank_lines: ビジュアル行が属するブロック全体の Markdown 行範囲を取得
/// - Mermaid/画像: テキストスパンを持たないブロックの位置対応
/// - フォールバック: TextSpan の解決が失敗した場合のブロックレベル fallback
#[derive(Debug, Clone)]
pub struct BlockSpan {
    pub md_range: Range<usize>,
    pub typst_range: Range<usize>,
}
```

---

## 公開 API: BoundIndex

```rust
/// コンパイル済みドキュメントに束縛された ContentIndex。
///
/// (A) Markdown ↔ (C) Frame tree の変換 API を提供する。
/// 内部の (B) Typst content_text 層は隠蔽される。
pub struct BoundIndex<'a> {
    index: &'a ContentIndex,
    source: &'a Source,
    content_offset: usize,
    md_source: &'a str,
}

/// Markdown ソース内の位置情報。
pub struct MdPosition {
    /// Markdown バイトオフセット（テキストスパン単位の精度）。
    pub offset: usize,
    /// 所属するブロックの Markdown バイト範囲。
    pub block_range: Range<usize>,
}

impl<'a> BoundIndex<'a> {
    pub fn new(
        index: &'a ContentIndex,
        source: &'a Source,
        content_offset: usize,
        md_source: &'a str,
    ) -> Self;

    // ========== (C) → (A): Frame tree → Markdown ==========

    /// Frame tree の Span を Markdown 位置に解決する。
    ///
    /// 変換チェーン (ContentIndex 内部で完結):
    ///   Span → Source::range() → main.typ byte range → content_offset 減算
    ///   → TextSpan 検索 → エスケープ補正 → Markdown byte offset
    ///
    /// 呼び出し側は Span を渡すだけ。(B) Typst content_text の存在を知る必要はない。
    pub fn resolve_span(&self, span: Span) -> Option<MdPosition>;

    /// resolve_span の結果を 1-based 行番号に変換するショートカット。
    pub fn resolve_span_to_line(&self, span: Span) -> Option<usize>;

    /// Span が属するブロックの Markdown 行範囲 (1-based, inclusive) を返す。
    pub fn resolve_span_to_block_lines(&self, span: Span) -> Option<(usize, usize)>;

    // ========== (A) → (C): Markdown → Frame tree ==========

    /// Markdown バイト範囲を、Frame tree walk で使用するターゲット範囲に変換する。
    ///
    /// 戻り値は main.typ 内のバイト範囲のリスト。Frame tree walk 時に各グリフの
    /// Source::range(span) と照合し、重なりがあればそのグリフがハイライト対象。
    ///
    /// 変換チェーン (ContentIndex 内部で完結):
    ///   Markdown byte range → TextSpan 検索 → エスケープ補正
    ///   → Typst content_text byte range → content_offset 加算 → main.typ byte range
    ///
    /// 呼び出し側は Markdown 範囲を渡し、main.typ 範囲を受け取る。
    /// (B) Typst content_text の存在を知る必要はない。
    pub fn md_to_span_targets(&self, md_ranges: &[Range<usize>]) -> Vec<Range<usize>>;
}
```

### resolve_span の内部動作

```
入力: Span (Frame tree のグリフから取得)
  │
  ├─ Source::range(span) → main.typ byte range
  ├─ main_range.start < content_offset? → None (テーマ由来)
  ├─ typst_offset = main_range.start - content_offset
  │
  ├─ TextSpan を二分探索 (typst_range で検索)
  │   ├─ 見つかった場合:
  │   │   ├─ kind=Code: 1:1 変換 → md_range.start + (typst_offset - typst_range.start)
  │   │   ├─ kind=Plain: エスケープ走査で補正 → 正確な md offset
  │   │   ├─ kind=Math: 内部推定不可 → md_range.start (範囲先頭)
  │   │   ├─ kind=Break: md_range.start
  │   │   └─ kind=Opaque: md_range.start
  │   │
  │   └─ block_range: TextSpan を含む BlockSpan の md_range
  │
  └─ TextSpan が見つからない場合:
      └─ BlockSpan のみで検索 → block_range のみ、offset は block_range.start
```

### md_to_span_targets の内部動作

```
入力: Vec<Range<usize>> (Markdown byte ranges, regex マッチ結果)
  │
  各 md_range について:
  ├─ spans_overlapping_md(md_range) → 重なる TextSpan を列挙
  │   (md_order を使った二分探索)
  │
  各 TextSpan について:
  ├─ md_range と TextSpan.md_range の重なり部分を計算
  ├─ 重なり部分の md offset → typst offset に変換
  │   ├─ kind=Code: 1:1 変換
  │   ├─ kind=Plain: エスケープ走査で補正
  │   ├─ kind=Math: 範囲全体 (部分マッチ不可 → typst_range 全体を返す)
  │   ├─ kind=Opaque: スキップ (テキストマッチング対象外)
  │   └─ kind=Break: スキップ
  │
  ├─ typst offset + content_offset → main.typ byte range
  └─ 出力に追加
```

---

## 変換の困難さと SpanKind の対応表

| 変換の困難さ | SpanKind | 対処（ContentIndex 内部） | 外から見た挙動 |
|--------------|----------|---------------------------|---------------|
| テキストエスケープ (`#`→`\#`) | Plain | エスケープ走査で文字位置を補正 | 正確な位置 |
| コード（変換なし） | Code | 1:1 バイト対応 | 正確な位置 |
| 数式 (LaTeX→Typst math) | Math | 範囲全体としてマッピング | 数式全体が対象 |
| Mermaid (Markdown→SVG→`#image`) | Opaque | ブロック範囲のみ | ブロック全体が対象 |
| 画像 (`![](path)`→`#image`) | Opaque | ブロック範囲のみ | ブロック全体が対象 |
| テーブル構造の再編成 | Plain (セル内) | セル内テキストのマッピング | セル内テキスト位置 |
| リストインデント挿入 | Break | Break スパンで対応 | 改行位置は正確 |
| Typst ブロック間セパレータ `\n` | (TextSpan なし) | TextSpan がカバーしない領域 | BlockSpan fallback |

---

## 構築: markdown_to_typst() への変更

### 原則

pulldown_cmark の `into_offset_iter()` が各イベントに Markdown バイト範囲を付与する。
出力への書き込み前後の `output.len()` と組み合わせれば、TextSpan を追加コストほぼゼロで記録できる。

### 記録ポイント

| イベント | SpanKind | 備考 |
|----------|----------|------|
| `Event::Text(text)` | Plain | コードブロック内なら Code。テーブルセル内は遅延処理 |
| `Event::Code(code)` | Code | インラインコード |
| `Event::SoftBreak` | Break | リスト内はインデント込み |
| `Event::HardBreak` | Break | `\ \n` |
| `Event::InlineMath(latex)` | Math | |
| `Event::DisplayMath(latex)` | Math | |
| `Event::End(Image)` / Mermaid | Opaque | `#image()` 出力の範囲を記録 |

**記録しないもの:**
- インラインマークアップタグ (`#strong[`, `]` 等) — 構造テキスト
- 画像の alt テキスト — suppress されている
- ブロック間の空行・セパレータ — TextSpan がカバーしない隙間として扱う

### コードブロックの遅延処理

コードブロック内容は `code_block_buf` にバッファされ `End(CodeBlock)` で出力される。
バッファ内の相対オフセットを記録し、End 時にフェンスヘッダ長を加算して確定する:

```rust
// Text イベント時 (in_code_block = true):
code_spans_pending.push((md_range.clone(), buf_start..buf_end));

// End(CodeBlock) 時:
let content_start = output.len();  // フェンスヘッダ直後
output.push_str(&code_block_buf);
for (md_range, buf_range) in code_spans_pending.drain(..) {
    text_spans.push(TextSpan {
        md_range,
        typst_range: (content_start + buf_range.start)..(content_start + buf_range.end),
        kind: SpanKind::Code,
    });
}
```

### Mermaid ダイアグラムの扱い

Mermaid コードブロックは SVG にレンダリングされ、Typst 上では `#image("mermaid_xxxx.svg")`
として埋め込まれる。SVG 内部のテキストへのマッピングは追跡しない。

```rust
// End(CodeBlock) で lang == "mermaid" かつ画像が利用可能:
let typst_start = output.len();
output.push_str(&typst_image(&key));
let typst_end = output.len();
text_spans.push(TextSpan {
    md_range: md_range.clone(),        // Mermaid コードブロック全体
    typst_range: typst_start..typst_end,  // #image("...") 呼び出し
    kind: SpanKind::Opaque,
});
```

highlight で "mermaid" ブロック内のテキストを検索した場合、Opaque スパンはスキップされる。
yank では BlockSpan 経由でブロック全体の Markdown テキスト（元の mermaid コード）が返る。

### テーブルセルの遅延処理

テーブルは `cell_buf` → `table_cells` → `End(Table)` で一括出力。
コードブロックと同じパターンで遅延処理する:

```rust
// セル内 Text イベント → cell_spans_pending に記録
// End(Table) 時 → 各セルの出力位置を確定し TextSpan を生成
```

### 戻り値の変更

```rust
pub fn markdown_to_typst(
    markdown: &str,
    available_images: Option<&HashSet<String>>,
) -> (String, ContentIndex)    // SourceMap → ContentIndex に変更
```

---

## Yank への適用

### Before (現状)

```rust
// tile.rs: resolve_md_line_range()
Span → Source::range(span) → content_offset 減算
→ SourceMap.find_by_typst_offset() → BlockMapping
→ byte_offset_to_line() → md_line_range (ブロック)
→ ニューライン数ヒューリスティック → md_line_exact (行、条件付き)
```

ニューライン数の一致チェック、コードブロックのフェンス補正、段落セパレータの検出など、
**変換の困難さが tile.rs に漏れ出している。**

### After (ContentIndex)

```rust
// tile.rs: BoundIndex を使う
let pos = bound_index.resolve_span(span)?;
let exact_line = byte_offset_to_line(md_source, pos.offset);
let block_lines = (
    byte_offset_to_line(md_source, pos.block_range.start),
    byte_offset_to_line(md_source, pos.block_range.end.saturating_sub(1)),
);
```

tile.rs は `resolve_span()` を呼ぶだけ。エスケープ補正もニューライン数チェックも
フェンス補正もすべて ContentIndex 内部に閉じる。

### VisualLine の変更

```rust
pub struct VisualLine {
    pub y_pt: f64,
    pub y_px: u32,
    /// Markdown バイトオフセット（テキストスパン単位の精度）。
    /// None = テーマ由来テキスト。
    pub md_offset: Option<usize>,
    /// 所属ブロックの Markdown バイト範囲。
    /// None = テーマ由来テキスト。
    pub md_block_range: Option<Range<usize>>,
}
```

`md_line_range: Option<(usize, usize)>` と `md_line_exact: Option<usize>` を廃止。
行番号への変換は呼び出し側の責務（`byte_offset_to_line()` を使う）。

---

## Highlight への適用

### Before (現状)

```rust
// highlight.rs: find_highlight_rects()
Frame tree walk → 各 TextItem.text に regex → グリフ位置からピクセル矩形
// SourceMap をバイパス。テキスト再検索。数式・Mermaid で破綻。
```

### After (ContentIndex)

**親プロセス側 (viewer/tiles.rs):**

```rust
// 1. Markdown で regex マッチ
let md_matches: Vec<Range<usize>> = re.find_iter(md_source)
    .map(|m| m.start()..m.end())
    .collect();

// 2. ContentIndex で main.typ byte ranges に変換
let targets = bound_index.md_to_span_targets(&md_matches);

// 3. fork child に targets を送信
send_request(Request::FindHighlightRects { idx, targets });
```

**fork child 側 (highlight.rs):**

```rust
/// Span ベースのハイライト矩形検索。
///
/// target_ranges は main.typ 内のバイト範囲。
/// 各グリフの Span を解決し、target_ranges との重なりでハイライトを判定する。
pub fn find_highlight_rects_by_spans(
    frame: &Frame,
    target_ranges: &[Range<usize>],
    source: &Source,
    ppi: f32,
) -> Vec<HighlightRect>
```

Frame tree の各 TextItem の各グリフについて:
1. `Source::range(glyph.span)` → main.typ byte range
2. `target_ranges` のいずれかと重なるか判定
3. 重なるグリフ群からピクセル矩形を生成

**利点:**
- **数式:** SpanKind::Math のスパンは typst_range 全体が target に含まれる → Frame tree 上の数式グリフの Span が自動的にマッチ。テキスト内容の一致は不要
- **TextItem 境界またぎ:** 各グリフを個別に判定するため、TextItem 境界は無関係
- **Mermaid:** SpanKind::Opaque はスキップされる → 画像上にハイライトは出ない（正しい挙動）

### HighlightSpec の変更

```rust
// Before:
pub struct HighlightSpec {
    pub pattern: String,
    pub case_insensitive: bool,
}

// After:
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HighlightSpec {
    /// main.typ 内のバイト範囲リスト。親プロセスで事前計算済み。
    pub target_ranges: Vec<Range<usize>>,
}
```

regex 実行と ContentIndex アクセスは親プロセスで完結。
fork child には解決済みの範囲リストのみ送信される。

---

## オフセット補間: Plain テキストのエスケープ補正

`SpanKind::Plain` では `escape_typst()` により `#` → `\#` 等の膨張が起きる。
ContentIndex 内部で Markdown offset ↔ Typst offset を正確に変換する必要がある。

### 採用: 線形走査（方法 B）

TextSpan 内のテキストは短い（数十〜数百バイト）。
変換のたびに Markdown テキストを先頭から走査し、エスケープ文字を数えて補正する。
追加メモリ不要、実装は `escape_typst()` のロジックを共有。

```rust
/// Plain スパン内の Markdown offset → Typst offset 変換。
/// Markdown テキストを走査し、エスケープ文字（`#`, `*`, `_` 等）で
/// Typst 側のバイト位置を +1 ずつ進める。
fn md_to_typst_offset_in_plain_span(md_text: &str, md_local_offset: usize) -> usize {
    let mut typst_offset = 0;
    for (i, ch) in md_text.byte_indices() {
        if i >= md_local_offset { break; }
        typst_offset += if needs_escape(ch) { 2 } else { ch.len_utf8() };
    }
    typst_offset
}
```

逆方向（Typst → Markdown）も同様の走査で実装。

---

## IPC プロトコルの変更

### Before

```
親 → child: HighlightSpec { pattern, case_insensitive }
child → 親: Vec<HighlightRect>
```

### After

```
親 → child: HighlightSpec { target_ranges: Vec<Range<usize>> }
child → 親: Vec<HighlightRect>
```

fork child の変更:
- `find_highlight_rects()` → `find_highlight_rects_by_spans()` に置換
- `Source` 参照が必要 → fork child は既に `MluxWorld` を保持しているため `world.main_source()` が使える

---

## 削除されるコード

| 場所 | 削除対象 | 理由 |
|------|----------|------|
| `src/pipeline/markup.rs` | `SourceMap`, `BlockMapping`, `find_by_typst_offset()` | ContentIndex に統合 |
| `src/tile.rs` | `resolve_md_line_range()`, `MdLineInfo` | BoundIndex.resolve_span() に置換 |
| `src/tile.rs` | ニューライン数ヒューリスティック全体 | ContentIndex 内部に吸収 |
| `src/tile.rs` | `SourceMappingParams` | BoundIndex に置換 |
| `src/highlight.rs` | `walk_frame()`, `collect_text_rects()` の regex ロジック | Span ベースに全面書き換え |

---

## 影響範囲

| ファイル | 変更内容 |
|----------|----------|
| `src/pipeline/markup.rs` | SourceMap 削除、ContentIndex 構築、TextSpan 記録 |
| `src/pipeline/mod.rs` | pub use 変更 |
| `src/pipeline/build.rs` | ContentIndex の受け渡し |
| `src/tile.rs` | SourceMappingParams → BoundIndex。VisualLine フィールド変更。resolve_md_line_range 削除 |
| `src/highlight.rs` | regex → Span ベースに全面書き換え |
| `src/fork_render/mod.rs` | HighlightSpec 変更、IPC 更新 |
| `src/viewer/tiles.rs` | BoundIndex 使用、md_to_span_targets で事前計算 |
| `src/viewer/mode_search.rs` | highlight_spec() → target_ranges 生成 |
| `src/viewer/mode_normal.rs` | yank API 変更に追従 |
| `src/viewer/mod.rs` | ContentIndex/BoundIndex の保持 |
| `tests/integration.rs` | SourceMap テスト → ContentIndex/BoundIndex テスト |

---

## 実装順序

一括移行。段階的マイグレーションではない。

### Step 1: ContentIndex + BoundIndex のデータ構造と構築

- `ContentIndex`, `BoundIndex`, `TextSpan`, `SpanKind`, `BlockSpan`, `MdPosition` を定義
- `markdown_to_typst()` で TextSpan/BlockSpan を記録し ContentIndex を返す
  - Plain, Code, Math, Break, Opaque の全種別
  - コードブロック・テーブルの遅延処理
  - Mermaid の Opaque 記録
- 既存の BlockMapping/SourceMap を削除
- BoundIndex の `resolve_span()`, `md_to_span_targets()` を実装
- ユニットテスト: 各ブロックタイプで TextSpan/BlockSpan の正しさを検証

### Step 2: Yank の移行

- `resolve_md_line_range()` と ニューライン数ヒューリスティックを削除
- `SourceMappingParams` → `BoundIndex` に置換
- `VisualLine` のフィールド変更 (`md_offset`, `md_block_range`)
- `yank_exact()`, `yank_lines()`, `extract_urls()` を新フィールドに対応
- 既存の yank 統合テスト (integration.rs) がすべて通ることを確認

### Step 3: Highlight の移行

- `find_highlight_rects_by_spans()` を実装（Span ベース）
- `HighlightSpec` を `target_ranges` ベースに変更
- 親プロセスで `md_to_span_targets()` 呼び出し → fork child に送信
- fork child 側の対応 (`Source` 参照の追加)
- ハイライトテスト更新

### Step 4: クリーンアップ

- 旧コードの完全削除
- docs 更新

---

## リスクと制約

### 数式のハイライト粒度

SpanKind::Math は範囲全体のマッピングのみ。`$E = mc^2$` で `mc` を検索しても
数式全体がハイライトされる。数式内部の部分マッチは将来課題。

### Mermaid のハイライト

SpanKind::Opaque により、Mermaid ブロック内テキストの検索マッチは
ハイライト対象にならない。yank ではブロック全体（元の Mermaid コード）が返る。
SVG 内部のテキスト位置追跡は行わない。

### テーブルセルのマッピング

テーブルは構造が再編成されるが、セル内テキストは TextSpan (Plain) として記録される。
セル単位での構造対応は必要ない — テキストレベルのマッピングで十分。

### glyph.span 解決のパフォーマンス

新 highlight では全グリフの `Source::range(span)` 呼び出しが必要。
`Source::range()` は O(log n) であり、タイルあたりのグリフ数は数百〜数千程度。
実用上問題ないと予想するが、プロファイリングで検証する。
パフォーマンスが問題になった場合、main.typ byte range のソート済みリストと
グリフ Span のソート済みリストのマージ走査で O(n) に改善可能。

### SoftBreak のインデント

リスト内の SoftBreak で挿入されるインデントスペースは Markdown に存在しない。
SpanKind::Break の TextSpan で typst_range がインデント込み、md_range が改行 1 文字分。
Break スパンはオフセット補間の対象外であり、影響しない。
