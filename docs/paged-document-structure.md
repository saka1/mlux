# PagedDocument フレーム構造 詳細解析

mlux の Typst コンパイル結果（`PagedDocument`）の内部構造を実機ダンプに基づき記録する。
将来の Phase 4（Kitty Graphics Protocol）および行・ブロック座標に依存する機能の設計資料。

## 調査環境

- typst 0.14.2（crates.io 公開版）
- 入力: `tests/fixtures/07_full_document.md`（見出し・段落・コードブロック・テーブル・引用・リスト・リンク・取り消し線を含む）
- テーマ: `themes/catppuccin.typ`
- ページ幅: 660pt, `height: auto`, margin: 40pt

## コンパイルからレンダリングまでの分離

Typst はコンパイルとレンダリングを明確に分離している。

```
typst::compile::<PagedDocument>(&world)
  → PagedDocument                    ← レイアウト済み中間表現（ここで止められる）

typst_render::render(&page, ppi/72)
  → tiny_skia::Pixmap                ← ラスタライズ（PNG化はここで初めて行われる）
```

`PagedDocument` を得た時点で、以降の出力先は完全にアプリケーション任せ:

| 出力先 | クレート |
|---|---|
| PNG | `typst-render` → `tiny_skia::Pixmap` |
| PDF | `typst-pdf` |
| SVG | `typst-svg` |
| HTML | `typst-html` |
| 独自処理 | `Frame` を直接ウォーク |

## height: auto による無限縦長ページ

`PagedDocument` は名前の通りページ区切りのある文書を想定しているが、
`#set page(height: auto)` を指定すると**コンテンツに合わせて高さが自動伸長**する。
この場合 `document.pages` の長さは常に1で、その1ページが全コンテンツを含む。

mlux では catppuccin.typ の `#set page(height: auto)` により、
スクロールビュー的な無限縦長フレームとして使用している。

## Document トレイト

`typst::compile()` は型パラメータで出力形式を選択する:

```rust
// ページ文書（PDF/PNG/SVG向け）
typst::compile::<PagedDocument>(&world)

// HTML文書
typst::compile::<HtmlDocument>(&world)
```

`Document` トレイトは sealed（外部実装不可）で、上記2つのみ。
mlux の用途（Typst レイアウトエンジンを活かしたラスタライズ）には `PagedDocument` が適切。

## フレームツリー構造

### 全体像

```
PagedDocument
  └── pages: Vec<Page>
        └── frame: Frame
              ├── size: Size (幅, 高さ)
              └── items: Vec<(Point, FrameItem)>  ← 絶対座標 + アイテム
```

`Frame::items()` で `Iter<(Point, FrameItem)>` を取得できる。

### FrameItem の種類

```rust
pub enum FrameItem {
    Group(GroupItem),          // ネストしたFrame（変換・クリップ付き）
    Text(TextItem),            // テキストラン（shaped text の1区間）
    Shape(Shape, Span),        // 矩形・線・ベジェ曲線（背景・罫線・下線等）
    Image(Image, Size, Span),  // 画像
    Link(Destination, Size),   // 内部/外部リンク領域
    Tag(Tag),                  // introspection用マーカー（レイアウト情報なし）
}
```

### GroupItem（ネストしたサブフレーム）

```rust
pub struct GroupItem {
    pub frame: Frame,           // 子Frame
    pub transform: Transform,   // 変換行列（回転・スケール等）
    pub clip: Option<Curve>,    // クリッピング領域
    pub label: Option<Label>,   // ラベル
    pub parent: Option<FrameParent>,
}
```

ブロック要素（見出し・コードブロック・テーブル・引用等）は `Group` として出現する。
段落もテキストが1行で収まる場合は `Group` に格納される。
テーブルの行・セル、リストの項目も `Group` でネストされる。

### TextItem（テキストラン）

```rust
pub struct TextItem {
    pub font: Font,
    pub size: Abs,           // フォントサイズ (pt)
    pub fill: Paint,         // 文字色
    pub stroke: Option<FixedStroke>,
    pub lang: Lang,
    pub region: Option<Region>,
    pub text: EcoString,     // プレーンテキスト
    pub glyphs: Vec<Glyph>,  // グリフ配列（文字数と1:1とは限らない）
}
```

- `TextItem::width()` — テキストランの横幅
- `TextItem::height()` — テキストランの高さ
- `TextItem::bbox()` — バウンディングボックス（`Rect`）

### Glyph（個別グリフ）

```rust
pub struct Glyph {
    pub id: u16,           // フォント内グリフID
    pub x_advance: Em,     // 横送り量
    pub x_offset: Em,      // 横オフセット
    pub y_advance: Em,     // 縦送り量
    pub y_offset: Em,      // 縦オフセット
    pub range: Range<u16>, // TextItem.text 内のバイト範囲（UTF-8）
    pub span: (Span, u16), // ソースコード上の位置
}
```

`Em` 単位は `Abs` に変換可能: `glyph.x_advance.at(text_item.size)` → `Abs` (pt)

### 座標系

- 原点: ページ左上
- X: 右方向が正
- Y: 下方向が正（**テキストのグリフ座標系だけ Y-up** なので `TextItem::bbox()` 内部で反転処理あり）
- 単位: pt (1pt = 1/72 inch)
- ピクセル座標への変換: `pt値 × (ppi / 72.0)`

`(Point, FrameItem)` の `Point` はその `Frame` 内での相対座標。
ネストした `Group` の場合、絶対座標は親の `Point` + 自身の `Point` を加算して求める。

## 実機ダンプ結果の分析

入力: `tests/fixtures/07_full_document.md`
結果: 1ページ, 660.0pt × 929.3pt

### トップレベルのブロック要素マッピング

以下は `pages[0].frame.items` の直接の子要素から `Tag` を除外したもの。

| Y座標 (pt) | サイズ | 要素 | Markdown上の対応 |
|---|---|---|---|
| 40.0 | 366×18 | Group | `# Rustにおけるエラーハンドリング` (H1) |
| 79.6 | 80×15 | Group | `## はじめに` (H2) |
| 103.3 | 580×11.6 | Group | 段落（1行目: `Rustのエラーハンドリングは...`） |
| 126.9 | 580×9 | Group | 段落（2行目: 折り返し `コンパイル時に...`） |
| 157.5 | 120×15 | Group | `## 基本パターン` (H2) |
| 186.9 | 70×12 | Group | `### ? 演算子` (H3) |
| 204.9 | — | Text (直接) | `最も一般的なパターン:` |
| 228.3 | 580×102 | Group | コードブロック（rustソース） |
| 344.7 | 128×12 | Group | `### カスタムエラー型` (H3) |
| 362.7 | 302×107.8 | Group | テーブル（3列×4行） |
| 499.3 | 572×81.4 | Group | blockquote（ネストした引用含む） |
| 602.4 | 160×15 | Group | `## エラー処理の手順` (H2) |
| 626.0 | 188×56.2 | Group | 順序付きリスト（3項目） |
| 696.6 | — | Text (直接) | `避けるべきパターン:` |
| 720.0 | 284×56.2 | Group | 箇条書きリスト（3項目） |
| 797.8 | 60×15 | Group | `## まとめ` (H2) |
| 821.5 | 580×9 | Group | 段落（リンク含む1行目） |
| 842.5 | 580×9 | Group | 段落（折り返し2行目） |
| 865.9 | — | Shape | 水平線 (`---`) |
| 889.3 | — | Text (直接) | `最終更新: 2025年2月`（斜体） |

### 各ブロック要素の内部構造

#### 見出し (Heading)

```
Group (40.0, 40.0)pt  366.0x18.0pt      ← H1 の Group
  Text  (40.0, 58.0)pt  size=24.0pt  "Rust"
  Text  (88.0, 58.0)pt  size=24.0pt  "における"
  Text  (190.0, 58.0)pt  size=24.0pt  "エラーハンドリング"
```

- 見出しレベルはフォントサイズで識別可能: H1=24pt, H2=20pt, H3=16pt
- テーマの `#show heading` で設定したサイズに対応
- Group のサイズがバウンディングボックスを示す

#### 段落 (Paragraph)

```
Group (40.0, 103.3)pt  580.0x11.6pt     ← 1行目
  Text  (40.0, 114.9)pt  size=12.0pt  "Rust"
  Text  (64.0, 114.9)pt  size=12.0pt  "の"
  ...
Group (40.0, 126.9)pt  580.0x9.0pt      ← 2行目（折り返し）
  Text  (40.0, 135.9)pt  size=12.0pt  "コンパイル"
  ...
```

- **折り返しが発生すると複数の Group に分かれる**
- 各 Group は1行分のテキストランを含む
- 同一段落に属する行を識別するには、Group 間のY間隔が行送り（leading）と一致するかで判断可能
- 段落内行間隔 ≈ 12pt (本文) + leading (1em = 12pt) = 実際のY差 ≈ 23.6pt
- 段落間は `above`/`below` の値分だけ追加の間隔がある

#### CJK テキストの分割

```
Text  (40.0, 114.9)  "Rust"     ← ラテン文字はまとめて1ラン
Text  (64.0, 114.9)  "の"       ← 日本語は1文字ずつ分離されることがある
Text  (79.0, 114.9)  "エラーハンドリング"  ← カタカナは連続するとまとまる
```

Typst のシェイパーはフォント切り替え境界でテキストランを分割するため、
ラテン文字→CJK、CJK→ラテン文字の境界で `TextItem` が分かれる。
CJK 文字同士でも個別グリフに分かれるケースがある（組版エンジンの仕様）。

#### コードブロック (Code Block)

```
Group (40.0, 228.3)pt  580.0x102.0pt    ← コードブロック全体
  Shape (40.0, 228.3)pt                  ← 背景矩形（fill: #313244, radius: 6pt）
  Group (52.0, 240.3)pt  373.3x78.0pt   ← コード本体（inset: 12pt 分オフセット）
    Group (52.0, 240.3)pt  ...x7.6pt     ← 1行目: fn read_config(...)
    Group (52.0, 257.9)pt  ...x7.6pt     ← 2行目: let content = ...
    Group (52.0, 275.5)pt  ...x7.6pt     ← 3行目: let config = ...
    Group (52.0, 293.1)pt  ...x7.6pt     ← 4行目: Ok(config)
    Text  (52.0, 318.3)pt  "}"           ← 最終行（Groupでなく直接Text）
```

- 最外の `Group` がブロック全体。`Shape` が背景矩形（角丸）。
- コード各行が個別の `Group` に分かれている
- フォントサイズ: 10pt（テーマの `raw.where(block: true)` 設定）
- テーマの `inset: 12pt` により、コードテキストは背景から12pt内側にオフセット

#### テーブル (Table)

```
Group (40.0, 362.7)pt  302.2x107.8pt   ← テーブル全体
  Shape ... (×12)                       ← 罫線・背景（セル背景、行区切り線、列区切り線）
  Tag → Text "クレート"                 ← ヘッダ行セル
  Tag → Text "特徴"
  Tag → Text "用途"
  Group (40.0, 387.8)pt  302.2x27.6pt  ← データ行1
    Group ... → Shape + Text "thiserror" ← インラインコード（背景Shape付き）
    Text "derive マクロ"
    Text "ライブラリ"
  Group (40.0, 415.4)pt  ...            ← データ行2
  Group (40.0, 443.0)pt  ...            ← データ行3
```

- `Shape` が大量に出現: セルの罫線 (`stroke: 0.5pt + rgb(#585b70)`) と
  ヘッダ行の背景色 (`fill: rgb(#313244)`)
- ヘッダ行のセルは `Tag` + `Text` としてフラットに出現
- データ行は `Group` でまとめられ、各セルの内容がネスト
- テーブル内のインラインコードは `Group(Shape + Text)` の入れ子

#### 引用ブロック (Blockquote)

```
Group (40.0, 499.3)pt  572.3x81.4pt     ← 引用ブロック全体
  Shape (40.0, 499.3)pt                   ← 左ボーダー（stroke: left: 3pt + #89b4fa）
  Group (56.0, 507.3)pt  556.3x11.6pt    ← 引用テキスト1行目
    Text "Note" (bold)
    Text ": ライブラリでは ..."
    Group ... "thiserror"                  ← インラインコード
    ...
  Group (56.0, 547.7)pt  286.0x25.0pt    ← ネストした引用（>> で始まる部分）
    Shape (56.0, 547.7)pt                  ← 内側の左ボーダー
    Group (72.0, 555.7)pt ...              ← ネストした引用のテキスト
```

- 最外の `Group` が引用ブロック全体
- `Shape` が左ボーダー線（テーマの `stroke: (left: 3pt + rgb("#89b4fa"))` に対応）
- ネストした引用は内部にさらに `Group` + `Shape` がネスト
- `inset: (left: 16pt)` により、テキストが左にオフセット

#### リスト (List)

```
Group (40.0, 626.0)pt  188.0x56.2pt    ← 順序付きリスト全体
  Text "1."                              ← 番号マーカー
  Group (58.0, 626.0)pt  170.0x9.0pt   ← 1項目のテキスト
  Text "2."
  Group (58.0, 647.0)pt  ...            ← 2項目のテキスト（インラインコード含む）
  Group (40.0, 670.6)pt  ...            ← 3項目（Text "3." + テキストが同じGroupに）
```

- リスト全体が1つの `Group`
- マーカー（`1.` / `•`）は `Text` として出現
- 各項目のテキストは `Group` に格納
- 項目のインデントは `x=58.0pt`（マーカー幅分のオフセット）

#### 水平線 (Horizontal Rule)

```
Shape (40.0, 865.9)pt
```

単一の `Shape` として出現。罫線の描画。

#### リンク (Link)

```
Text  (602.0, 830.5)pt  "The "
Shape (602.0, 832.0)pt                   ← 下線
Link  (602.0, 815.5)pt  18.0x21.0pt     ← クリッカブル領域
```

- `Text` + `Shape`（下線）+ `Link`（クリック領域）の3つ組で出現
- テーマの `#show link: underline(...)` に対応
- `Link` はレンダリング上は不可視で、PDF等のインタラクション用

#### 取り消し線 (Strikethrough)

```
Group ... ← リスト項目内
  Group (52.0, 764.6)pt  44.1x11.6pt
    Shape ...                              ← インラインコード背景
    Text "panic!"
    Shape (56.0, 771.7)pt                  ← 取り消し線
  Text " による"
  Shape (96.1, 771.2)pt                    ← 取り消し線（続き）
  Text "強制終了"
  Shape (138.1, 771.2)pt                   ← 取り消し線（続き）
```

- 取り消し線は `Shape` としてテキストの上に重ねて描画
- テキストランの区切りごとに個別の `Shape` が出現

#### インラインコード (Inline Code)

```
Group (206.6, 103.3)pt  80.2x11.6pt    ← インラインコード `Result<T, E>`
  Shape (206.6, 103.3)pt                ← 背景矩形（fill: #313244, radius: 3pt）
  Text  (210.6, 112.9)pt  size=10pt  "Result<T, E>"
```

- `Group` 内に `Shape`（背景）+ `Text` のペア
- フォントサイズ 10pt、背景は丸角矩形
- テーマの `#show raw.where(block: false): box(fill: ..., inset: ..., radius: ...)` に対応

## Tag について

`Tag` は Typst の introspection システム（ページカウンタ、相互参照等）用の内部マーカー。
レイアウト情報は持たない。座標情報はあるが、レンダリングや座標取得の用途では**読み飛ばしてよい**。

ダンプ結果を見ると、ほぼ全てのブロック要素の前後に `Tag` が挿入されている。

## 行の特定方法

段落内の `Text` アイテムは、同じY座標を共有するものが同一行に属する。

```
Text (40.0, 114.9)pt "Rust"        ← Y=114.9 → 同一行
Text (64.0, 114.9)pt "の"          ← Y=114.9 → 同一行
Text (79.0, 114.9)pt "エラーハンドリング" ← Y=114.9 → 同一行
```

折り返しが発生すると、別の `Group` に分かれてY座標が変わる:

```
Group (40.0, 103.3)pt   ← 1行目のGroup
  Text (..., 114.9)pt   ← Y=114.9
Group (40.0, 126.9)pt   ← 2行目のGroup（折り返し後）
  Text (..., 135.9)pt   ← Y=135.9
```

**行の検出手順:**
1. `Frame::items()` を再帰ウォークし、全 `TextItem` の絶対座標を収集
2. Y座標でグループ化（同一Y座標 = 同一行）
3. X座標でソートすればテキストの出現順序が得られる

## ダンプ機能の使い方

`--dump` フラグで `PagedDocument` のフレームツリーを stderr に出力できる:

```bash
cargo run -- tests/fixtures/07_full_document.md --dump 2>&1 | head -50
```

実装: `src/render.rs` の `dump_document()` / `dump_frame()` 関数。
`Frame::items()` を再帰ウォークし、各アイテムの種類・座標・テキスト内容を出力する。

## 設計上の示唆

### ブロック境界の検出

トップレベルの `Group` がブロック要素に対応するため、
`pages[0].frame.items()` を走査して `FrameItem::Group` を列挙すれば
ブロック単位の座標（位置 + サイズ）が直接取得できる。

ただし:
- 段落テキストが折り返す場合、複数の `Group` に分かれる（同一段落の判定が必要）
- 短い段落は `Group` にならず直接 `Text` として出現するケースがある
- `Tag` が大量に挿入されるので、フィルタリングが必要

### ピクセル座標への変換

```
pixel_x = pt_x × (ppi / 72.0)
pixel_y = pt_y × (ppi / 72.0)
```

例: ppi=144 の場合、`pixel_per_pt = 2.0`
→ (40pt, 103.3pt) → (80px, 206.6px)

### Kitty Graphics Protocol との連携

フレームツリーから得た座標情報を使えば:
- ブロック単位のクロッピング → ブロックごとの部分PNG → 部分的な再送信
- 行座標の検出 → ターミナル行とTypstレイアウト行のマッピング
- テキスト検索 → `TextItem::text` からの全文検索 + 座標特定
