# 検索ハイライト表示の実現可能性調査

## 結論

**実現可能**。ただし2つの技術要素を組み合わせる必要がある。

| 要素 | 実現性 | 難易度 |
|------|--------|--------|
| KGPオーバーレイ（半透明画像の重ね表示） | ✓ 可能 | 低 |
| ソースマップによるピクセル座標特定 | ✓ 可能（要拡張） | 中〜高 |

---

## 1. KGPによるハイライトオーバーレイ

### 結論: 可能

Kitty Graphics Protocol は `z` パラメータによるレイヤリングをサポートしている。

```
z > 0  → テキストの上に描画
z = 0  → テキストと同レイヤー（デフォルト）
z < 0  → テキストの下に描画
```

現在のmluxのコンテンツ画像は `z` パラメータ未指定（デフォルト0）。
したがって `z=1` の半透明PNG画像を同じセル位置に配置すれば、コンテンツの上に重ねて表示できる。

### 実装イメージ

```
// 黄色半透明のハイライト矩形をPNGとして生成
let highlight_png = create_highlight_png(width_px, height_px, rgba(255, 255, 0, 80));

// KGPコマンドでコンテンツ画像の上に配置
// a=T: 転送+表示, z=1: コンテンツの上, x/y: ソース矩形内のオフセット
\x1b_Ga=t,f=100,i={highlight_id},t=d,q=2;{base64_data}\x1b\\
\x1b_Ga=p,i={highlight_id},z=1,c={cols},r={rows},C=1,q=2\x1b\\
```

### 注意点

- 半透明PNGの合成はKGP側でアルファブレンディングされる（Kitty本体が処理）
- ハイライト画像は小さい（数十×数十ピクセル程度）ので転送コストは無視できる
- スクロール時にはハイライト配置の更新が必要（`a=d` → 再配置）
- 同一Z-indexの画像はIDが小さい方が下になるため、ハイライト用IDはコンテンツIDより大きくする

---

## 2. ソースマップによるピクセル座標の特定

### 現状の能力

| マッピング | 可否 | 根拠 |
|------------|------|------|
| Markdown行 → ピクセルY座標 | ✓ あり | `VisualLine.y_px` |
| Markdown行+列 → ピクセルX座標 | ✗ 未実装 | グリフレベルの座標追跡が必要 |

### 必要な拡張: グリフレベルX座標の解決

Typst の Frame には、ハイライトに必要な情報がすべて含まれている:

```rust
// FrameItem::Text の中身
pub struct TextItem {
    pub font: Font,
    pub size: Abs,          // フォントサイズ（Em→Abs変換に使用）
    pub text: String,       // レンダリングされたテキスト
    pub glyphs: Vec<Glyph>, // 個別グリフ情報
}

pub struct Glyph {
    pub x_advance: Em,      // 横送り量 → .at(size) で Abs(pt) に変換可能
    pub x_offset: Em,       // 横オフセット
    pub range: Range<u16>,  // TextItem.text 内のバイト範囲
    pub span: (Span, u16),  // ソースコード上の位置
}
```

**つまり:**
- 各グリフの X 座標は `TextItem の絶対X位置 + Σ(前のグリフの x_advance)` で計算可能
- 各グリフの Y 座標は `TextItem の絶対Y位置` で取得可能
- 各グリフの幅は `x_advance.at(text_item.size).to_pt()` で取得可能
- 各グリフの高さはフォントサイズ（`text_item.size`）で近似可能

### マッピングチェーンの全体像

検索パターンのピクセル座標を特定するには、以下の2つのアプローチが考えられる。

#### アプローチA: Frame走査（推奨）

```
1. grep_markdown() で検索
   → SearchMatch { md_line, col_start, col_end } を取得

2. Markdown バイトオフセットを計算
   → md_line の行頭バイト位置 + col_start/col_end

3. SourceMap の BlockMapping を逆引き
   → md_byte_range を含む BlockMapping を見つける
   → 対応する typst_byte_range を取得

4. Frame ツリーを走査
   → 全 TextItem のグリフを調べる
   → glyph.span を Source::range() で解決し、typst_byte_range 内か判定
   → マッチしたグリフの絶対座標 + x_advance から矩形を計算

5. pt → px 変換
   → x_px = x_pt × (ppi / 72.0)
   → y_px = y_pt × (ppi / 72.0)
```

**利点**: Typstが実際にレンダリングした位置を正確に取得できる
**課題**: Frame走査のコスト（ただしタイル単位で絞り込めば軽量）

#### アプローチB: Typstマークアップ側でハイライト注入

```
1. 検索パターンの位置を特定（Markdown上）
2. Typstマークアップ生成時に、該当箇所を #highlight() で囲む
3. 再コンパイル → ハイライト済みPNGを直接取得
```

**利点**: ピクセル座標計算が不要（Typst自身がハイライト描画）
**課題**: 再コンパイルが必要（コスト大）、SourceMapの整合性維持が複雑

---

## 3. 推奨アプローチの詳細設計

### アプローチA（Frame走査）の実装ステップ

#### Step 1: SourceMap に逆引きメソッドを追加

```rust
impl SourceMap {
    /// Markdown バイトオフセットを含む BlockMapping を検索
    pub fn find_by_md_offset(&self, md_offset: usize) -> Option<&BlockMapping> {
        self.blocks.iter().find(|b| b.md_byte_range.contains(&md_offset))
    }
}
```

#### Step 2: Frame からハイライト矩形を抽出する関数

```rust
struct HighlightRect {
    x_px: u32,
    y_px: u32,
    width_px: u32,
    height_px: u32,
}

fn find_highlight_rects(
    frame: &Frame,
    source: &Source,
    content_offset: usize,
    target_typst_range: Range<usize>,  // ハイライト対象のTypstバイト範囲
    ppi: f64,
) -> Vec<HighlightRect> {
    // Frame ツリーを再帰走査し、target_typst_range 内の
    // グリフ群の矩形を収集する
}
```

#### Step 3: ハイライトPNG生成 + KGP配置

```rust
fn render_highlight_overlay(rects: &[HighlightRect], tile_idx: usize) -> Vec<u8> {
    // tiny-skia で半透明黄色の矩形を描画 → PNG エンコード
}
```

### 工数見積もり

| 作業 | 規模感 |
|------|--------|
| SourceMap 逆引き追加 | 小（数行） |
| Frame走査でグリフ座標計算 | 中（tile.rs の既存パターンを参考に） |
| ハイライトPNG生成 | 小（tiny-skia は既に依存に含まれる） |
| KGP オーバーレイ配置 | 小（既存の terminal.rs を参考に） |
| スクロール追従・ライフサイクル管理 | 中（既存の tile 配置管理を拡張） |
| 複数マッチのハイライト | 小（rects を複数描画するだけ） |

---

## 4. 想定される課題と対策

### 4.1 Markdown → Typst のテキスト変換による位置ズレ

Markdownの `**bold**` は Typstで `#strong[bold]` になる。
`col_start/col_end` はMarkdownソース上のバイト位置なので、
Typst側のグリフ位置と直接対応しない。

**対策**: グリフの `span` フィールドが Typst ソース位置を保持しており、
`Source::range(span)` → SourceMap 逆引きで Markdown 位置に戻せる。
したがって「Frame側から Markdown 位置を逆算」するアプローチが正しい。

具体的には:
1. 検索で `(md_line, col_start, col_end)` を取得
2. Frame を走査し、各グリフの span → Markdown バイト位置を解決
3. そのMarkdownバイト位置が検索マッチ範囲内かを判定
4. マッチしたグリフの座標からハイライト矩形を生成

### 4.2 タイル境界をまたぐハイライト

テキストがタイル境界をまたぐケースでは、`split_frame()` が
アイテムを両方のタイルにクローンしているため、
両タイルそれぞれでハイライト矩形を計算すれば対応可能。

### 4.3 コードブロック内のテキスト

コードブロック内のテキストは等幅フォントで描画されるため、
x_advance が均一でマッピングが容易。
ただし、シンタックスハイライトにより1行が複数の TextItem に分割される可能性がある。

### 4.4 Ghostty 互換性

Ghostty は KGP の z-index をサポートしている。
ただし、既知の `\x1b[2J` でのキャッシュ消去バグがあるため、
ハイライト画像の削除も `a=d,d=i` を使用すべき（既存のコンテンツ画像と同じ方式）。

---

## 5. 段階的実装の提案

### Phase 1: 行単位ハイライト（MVP）

VisualLine.y_px は既に利用可能なので、
**マッチした行全体を半透明帯でハイライト**するだけなら即座に実装可能。

- X座標計算不要（行全幅をハイライト）
- Frame走査不要（VisualLine.y_px + フォントサイズ高をハイライト矩形に）
- 検索結果ジャンプ時に「この行がマッチ」が視覚的に分かる

### Phase 2: 単語単位ハイライト

Frame走査を実装し、マッチした単語/パターンのみを正確にハイライト。

### Phase 3: 複数マッチの同時ハイライト

ビューポート内の全マッチ箇所を同時にハイライト（n/Nで「現在のマッチ」を強調色で区別）。

---

## 6. Q&A

### Q: Markdownの1行が折り返しや数式で複数レンダリング行になる場合、ハイライト座標は破綻しないか？

**破綻しない。** Phase 2のFrame走査アプローチはレイアウト行（VisualLine）を経由せず、
Typstが実際に配置したグリフの座標を直接読み取るため。

#### 折り返しのケース

Markdownの1行がTypstで折り返されると、複数の独立した TextItem に分割される:

```
Markdown 1行 → Typstで折り返し → 3つの TextItem

TextItem①  pos=(40pt, 100pt)  text="This is a very long paragraph"
TextItem②  pos=(40pt, 112pt)  text="that wraps across multiple"
TextItem③  pos=(40pt, 124pt)  text="lines"
```

各TextItemは独立した `(x, y)` 座標を持ち、グリフの `span` → `Source::range()` →
SourceMap で Markdown バイト位置に逆引きできる。
検索マッチが「wraps across」なら TextItem② 内のグリフ座標から矩形を確定できる。

折り返し行をまたぐマッチの場合は、複数の矩形を生成すればよい。

#### 数式のケース

数式は `FrameItem::Group` として複雑にネストされるが、テキスト部分は
`FrameItem::Text` であり各グリフに `span` と座標がある。
数式内の特定シンボルにマッチした場合でもグリフの `(x, y)` は取得可能。

ただし、SourceMap はブロック単位のマッピングなので、数式内の個別文字レベルの
Markdown↔Typst対応は `glyph.span` の `(Span, u16)` のうち `u16`（ノード内オフセット）
の活用が必要になる可能性がある。現在のmluxは `span.0` のみ使用しており、
ここは実装時に検証すべき点。

#### ケース別まとめ

| ケース | 破綻するか | 理由 |
|--------|-----------|------|
| 長い段落の折り返し | しない | 各折り返し行のTextItemが独立した座標を持つ |
| 折り返し行をまたぐマッチ | しない | 複数TextItemにまたがる矩形を複数生成 |
| 数式 | しない（ブロック精度） | グリフ座標は取得可能。文字単位精度は要検証 |
| テーブルセル内 | しない | セルもGroupとしてネスト、TextItemは座標を持つ |

#### 補足: Phase 1（行単位ハイライト）の場合

Phase 1 は VisualLine.y_px を使うため、折り返し後の2行目以降の位置は特定できない。
ただし「この付近にマッチがある」という視覚的手がかりにはなる。
**折り返し問題を本質的に解決するのは Phase 2 のFrame走査アプローチ。**
