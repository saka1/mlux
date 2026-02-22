# md2img v2: Typst組版エンジンによるMarkdown画像レンダラ

## 概要

Markdownテキストを入力し、Typstの組版エンジンでレイアウト・ラスタライズしてPNG画像を出力するCLIツール。Pure Rust実装。最終的にはKitty Graphics Protocolによるターミナルインライン表示 + Vimライクナビゲーションを目指す。本フェーズではTypstの組版品質検証とパイプラインの確立に集中する。

## アーキテクチャ

```
Markdown (入力)
    ↓ pulldown-cmark (CommonMark準拠パーサー)
Markdown AST
    ↓ 自前変換層
Typst マークアップ (文字列)
    ↓ theme.typ (show rule群) と結合
    ↓ typst クレート (compile)
Document (レイアウト済み内部表現)
    ↓ typst-render (tiny-skia)
PNG (出力)
```

### V1 (Pango) との差分

| 観点 | V1 (Pango + Cairo) | V2 (Typst) |
|---|---|---|
| ブロックレイアウト | 自前実装 | Typstに委譲 |
| 行分割 | Pango (UAX#14ベース) | Typst (Knuth-Plassベース) |
| テーブル | 自前2パス実装 | Typstのtable要素 |
| コードハイライト | Pygments + 手動描画 | Typstのraw要素 + テーマ |
| スタイル制御 | Pythonコード内で定義 | .typファイルで宣言的に定義 |
| 拡張性 | レンダラ改修が必要 | show rule / WASMプラグイン |
| 言語 | Python | Rust |
| 外部依存 | PyGObject, Pango, Cairo | なし (pure Rust) |

## 技術スタック

- Rust (edition 2021)
- `typst` — コンパイラ本体 (World トレイト実装が必要)
- `typst-render` — tiny-skiaベースのラスタライザ
- `typst-kit` — World実装のヘルパー (フォント解決等)
- `pulldown-cmark` — CommonMark準拠Markdownパーサー
- `clap` — CLI引数パーサー

## ディレクトリ構成

```
md2img/
├── Cargo.toml
├── src/
│   ├── main.rs           # CLIエントリポイント
│   ├── convert.rs        # Markdown AST → Typstマークアップ変換
│   ├── world.rs          # typst::World トレイト実装
│   └── render.rs         # compile → PNG出力
├── themes/
│   ├── catppuccin.typ    # ダークテーマ (デフォルト)
│   └── light.typ         # ライトテーマ
├── tests/
│   ├── fixtures/          # テスト用Markdownファイル群
│   │   ├── 01_paragraph_ja.md
│   │   ├── 02_heading.md
│   │   ├── 03_code_block.md
│   │   ├── 04_mixed_ja_en.md
│   │   ├── 05_table.md
│   │   ├── 06_blockquote_list.md
│   │   └── 07_full_document.md
│   ├── snapshots/         # 期待される出力画像 (目視確認後コミット)
│   └── integration.rs    # 各fixtureのレンダリングテスト
└── output/                # レンダリング結果の出力先
```

## 段階的実装計画

### Phase 1: 最小パイプライン — 日本語段落1つ

**ゴール**: Markdown→Typst→PNGのパイプラインを貫通させ、Typstの日本語組版品質を確認する。

**実装内容**:

#### 1a. World トレイト実装 (`world.rs`)

typst-kitを利用した最小実装:

```rust
struct MdWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    source: Source,           // メインソース (theme + content)
    theme_source: String,     // theme.typ の中身
}

impl World for MdWorld {
    fn library(&self) -> &LazyHash<Library>;
    fn book(&self) -> &LazyHash<FontBook>;
    fn main(&self) -> Source;
    fn source(&self, id: FileId) -> Result<Source>;
    fn file(&self, id: FileId) -> Result<Bytes>;
    fn font(&self, index: usize) -> Option<Font>;
    fn today(&self, offset: Option<i64>) -> Option<Datetime>;
}
```

フォント解決: システムフォントを検索。最低限 Noto Sans CJK JP が見つかること。

#### 1b. Markdown→Typst変換の最小実装 (`convert.rs`)

Phase 1では段落のみ:

```rust
fn markdown_to_typst(markdown: &str) -> String {
    let parser = pulldown_cmark::Parser::new(markdown);
    let mut output = String::new();
    for event in parser {
        match event {
            Event::Text(text) => output.push_str(&escape_typst(&text)),
            Event::SoftBreak => output.push('\n'),
            Event::HardBreak => output.push_str("\\ \n"),
            // Phase 1 では他は無視
            _ => {}
        }
    }
    output
}
```

Typstのエスケープが必要な文字: `#`, `*`, `_`, `` ` ``, `<`, `>`, `@`, `$`, `\`, `/`, `[`, `]` 等。

#### 1c. テーマファイル (`themes/catppuccin.typ`)

```typst
#set page(
  width: 800pt,
  height: auto,
  margin: 40pt,
  fill: rgb("#1e1e2e"),
)

#set text(
  font: ("Noto Sans CJK JP", "Noto Sans"),
  size: 12pt,
  fill: rgb("#cdd6f4"),
)

#set par(
  leading: 1em,
  justify: true,
  first-line-indent: 0pt,
)
```

#### 1d. レンダリング (`render.rs`)

```rust
fn render_to_png(world: &MdWorld, ppi: f32) -> Result<Vec<u8>> {
    let document = typst::compile(world).output?;
    let pixmap = typst_render::render(
        &document.pages[0].frame,
        ppi / 72.0,  // pixel_per_pt
    );
    pixmap.encode_png()
}
```

#### 1e. CLI (`main.rs`)

```
md2img input.md -o output.png [--width 800] [--theme catppuccin]
```

**検証テスト (01_paragraph_ja.md)**:

```markdown
日本語の組版において、禁則処理は最も基本的な要件の一つである。句読点「、」「。」
が行頭に来ないこと、開き括弧「（」が行末に来ないことなど、読みやすさを確保する
ためのルールが存在する。

「引用符で始まる段落」もまた、禁則処理の対象となる。全角ダッシュ——のような記号
や、三点リーダー……の扱いも重要である。

English text mixed with 日本語テキスト should have appropriate spacing between
scripts. This is known as 和欧間アキ and is a key quality indicator.
```

**品質チェックリスト**:

- [ ] パイプラインが貫通し、PNGが出力される
- [ ] 句読点が行頭に来ていないか
- [ ] 括弧類の禁則が正しいか
- [ ] 和欧間のスペーシングが自然か
- [ ] Knuth-Plass行分割による行末の揃い具合
- [ ] フォントの描画品質 (アンチエイリアス)
- [ ] V1 (Pango) との比較: 行分割品質に差があるか

---

### Phase 2: ブロック要素 + テーマのshow rule

**ゴール**: Markdownの主要ブロック要素をすべてサポートし、テーマのshow ruleで外観を制御する。

**実装内容**:

#### 2a. convert.rs 拡張

pulldown-cmarkのイベントをTypstマークアップに変換:

```
Markdown              → Typst
# Heading             → = Heading
## Heading            → == Heading
**bold**              → *bold*
*italic*              → _italic_
`code`                → `code`
[link](url)           → #link("url")[link]
![alt](src)           → #image("src", alt: "alt")
> blockquote          → #quote(block: true)[...]
- list item           → - list item
1. ordered            → + ordered item  (または #enum)
---                   → #line(length: 100%)
```code               → ```code```  (raw block)
| table |             → #table(...)
```

ネスト構造の扱いが最も複雑。pulldown-cmarkはイベントベース (Start/End) なのでスタックで管理:

```rust
struct ConvertState {
    output: String,
    stack: Vec<Container>,  // 現在のネスト状態
    list_depth: usize,
}

enum Container {
    BlockQuote,
    List { ordered: bool, index: usize },
    ListItem,
    Table { columns: Vec<Alignment> },
    TableHead,
    TableRow,
    TableCell,
}
```

#### 2b. テーマ拡張 (`themes/catppuccin.typ`)

```typst
// 見出し
#show heading.where(level: 1): it => {
  block(below: 0.8em, above: 1.2em,
    text(24pt, weight: "bold", fill: rgb("#cba6f7"), it.body))
}
#show heading.where(level: 2): it => {
  block(below: 0.6em, above: 1em,
    text(20pt, weight: "bold", fill: rgb("#f5c2e7"), it.body))
}
#show heading.where(level: 3): it => {
  block(below: 0.5em, above: 0.8em,
    text(16pt, weight: "bold", fill: rgb("#f5e0dc"), it.body))
}

// コードブロック
#show raw.where(block: true): it => {
  block(
    fill: rgb("#313244"),
    inset: 12pt,
    radius: 6pt,
    width: 100%,
    text(font: "JetBrains Mono", size: 10pt, it),
  )
}

// インラインコード
#show raw.where(block: false): it => {
  box(
    fill: rgb("#313244"),
    inset: (x: 4pt, y: 2pt),
    radius: 3pt,
    text(font: "JetBrains Mono", size: 10pt, it),
  )
}

// 引用ブロック
#show quote.where(block: true): it => {
  block(
    inset: (left: 16pt, y: 8pt),
    stroke: (left: 3pt + rgb("#89b4fa")),
    text(fill: rgb("#a6adc8"), it.body),
  )
}

// テーブル
#set table(
  stroke: 0.5pt + rgb("#585b70"),
  inset: 8pt,
  fill: (_, y) => if y == 0 { rgb("#313244") } else { none },
)

// リンク
#show link: it => {
  text(fill: rgb("#89b4fa"), underline(it))
}

// 水平線
#show line: it => {
  block(above: 1em, below: 1em,
    line(length: 100%, stroke: 0.5pt + rgb("#585b70")))
}
```

**検証テスト (02_heading.md 〜 06_blockquote_list.md)**:

- [ ] 見出し階層 h1〜h3 のサイズ・色の差が明確か
- [ ] コードブロックの背景・角丸・パディング
- [ ] シンタックスハイライト (Typstのraw要素が対応する言語)
- [ ] インラインコードの背景
- [ ] 引用ブロックの左バー + インデント
- [ ] ネストされた引用
- [ ] 順序なし / 順序付きリスト
- [ ] ネストされたリスト
- [ ] テーブルのヘッダー行装飾・罫線・アラインメント
- [ ] 日英混在テーブルの列幅
- [ ] リンクの色・下線
- [ ] 水平線

---

### Phase 3: エッジケースと変換品質の向上

**ゴール**: 実際の技術文書で破綻しない変換品質を達成する。

**実装内容**:

- Typstエスケープの網羅的処理
  - Markdown中の `#`, `$`, `@` 等がTypstの構文と衝突するケースの処理
  - HTMLインライン要素 (`<br>`, `<sup>` 等) の処理方針決定
- ネスト構造のエッジケース
  - 引用の中のコードブロック
  - リストの中のコードブロック
  - リストの中のテーブル (非標準だが現実に存在)
- 画像の扱い
  - ローカルパスの画像を World の file() 経由で供給
  - URLの画像は未対応でaltテキスト表示 (将来的にダウンロード対応)
- 長文ドキュメントの処理
  - `height: auto` で単一ページとして出力 → 巨大PNGになる問題
  - ページ分割するか、ブロック単位で分割画像にするかの設計判断

**検証テスト (07_full_document.md)**:

```markdown
# Rustにおけるエラーハンドリング

## はじめに

Rustのエラーハンドリングは `Result<T, E>` 型を中心に設計されている。
他の言語の例外機構と異なり、**コンパイル時にエラー処理を強制**する。

## 基本パターン

### `?` 演算子

最も一般的なパターン:

```rust
fn read_config(path: &str) -> Result<Config, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}
```

### カスタムエラー型

| クレート | 特徴 | 用途 |
|----------|------|------|
| `thiserror` | derive マクロ | ライブラリ |
| `anyhow` | 動的エラー型 | アプリケーション |
| `eyre` | カスタムレポート | CLIツール |

> **Note**: ライブラリでは `thiserror`、アプリケーションでは `anyhow` が
> 一般的な選択肢とされている。
>
> > ただし、この使い分けは絶対的なルールではない。

## まとめ

エラーハンドリングの設計は——プロジェクトの性質に応じて——柔軟に選択すべきである。
詳細は [The Rust Programming Language](https://doc.rust-lang.org/book/) を参照。

---

*最終更新: 2025年2月*
```

- [ ] 上記ドキュメントが全要素正しくレンダリングされるか
- [ ] コードブロック内の特殊文字が壊れないか
- [ ] 引用のネストが正しいか
- [ ] テーブルの列幅が自然か
- [ ] 全体のバランス・可読性

---

### Phase 4: ターミナル表示 (Kitty Graphics Protocol)

**ゴール**: レンダリング結果をターミナル上にインライン表示する。

**実装内容**:

- Kitty Graphics Protocol によるPNG画像送信
  - base64エンコード → `\x1b_Gf=100,t=d,a=T;{base64_data}\x1b\\`
  - チャンク分割送信 (4096バイト単位)
- ターミナル幅の検出 (`ioctl` TIOCGWINSZ) → ピクセル幅取得 → キャンバス幅決定
- ブロック単位の分割描画
  - ドキュメント全体を1枚の画像にするのではなく、ブロック要素ごとに分割
  - スクロール時に必要なブロックだけ再描画可能にする伏線

**検証**:

- [ ] Kitty / Ghostty / WezTerm で画像が表示されるか
- [ ] ターミナルリサイズ時に幅が追従するか
- [ ] 長文ドキュメントのスクロール表示

---

### Phase 5: Vimライクナビゲーション (将来構想)

Phase 4までの検証結果を踏まえて設計する。ここでは方向性のみ記載。

- TUIフレームワーク (`crossterm` ベース) によるキー入力捕捉
- Markdown ASTとの対応テーブル保持 (表示座標 ↔ AST位置)
- `j/k` — ブロック単位スクロール
- `v` + 移動 + `y` — 範囲選択 → OSC 52でクリップボード送信
- `/` — ASTテキストのインクリメンタルサーチ
- `q` — 終了

---

## テーマシステム設計

テーマは独立した `.typ` ファイルとして管理。ホスト側のコードを変更せずにスタイルを差し替えられる。

```
themes/
├── catppuccin.typ     # Catppuccin Mocha (デフォルトダーク)
├── light.typ          # ライトテーマ
└── custom.typ         # ユーザ定義
```

テーマファイルの責務:
- `#set page(...)` — キャンバスサイズ、背景色、余白
- `#set text(...)` — 本文フォント、サイズ、色
- `#set par(...)` — 行間、ジャスティフィケーション
- `#show heading: ...` — 各レベルの見出しスタイル
- `#show raw: ...` — コードブロック / インラインコード
- `#show quote: ...` — 引用ブロック
- `#set table(...)` — テーブルスタイル
- `#show link: ...` — リンク装飾

将来的にはWASMプラグインをテーマから呼び出すことで、任意の描画処理を追加可能。

## World トレイト実装の設計

```rust
// コンパイル時の仮想ファイルシステム
//
// main.typ (エントリポイント):
//   #import "theme.typ": *
//   #include "content.typ"
//
// theme.typ → themes/ディレクトリから読み込み or メモリ上
// content.typ → Markdown変換結果 (メモリ上で生成)

impl World for MdWorld {
    fn source(&self, id: FileId) -> Result<Source> {
        match id.vpath().as_rooted_path().to_str() {
            Some("/main.typ") => self.main_source(),
            Some("/theme.typ") => self.theme_source(),
            Some("/content.typ") => self.content_source(),
            _ => bail!("file not found"),
        }
    }

    fn file(&self, id: FileId) -> Result<Bytes> {
        // 画像等のバイナリファイル解決
        let path = id.vpath().as_rooted_path();
        fs::read(path).map(Bytes::from)
    }
}
```

## テスト用フィクスチャ

| ファイル | 検証対象 |
|---|---|
| 01_paragraph_ja.md | 日本語段落、禁則処理、和欧混在、Knuth-Plass行分割 |
| 02_heading.md | h1〜h4の階層、見出し直後の段落 |
| 03_code_block.md | Python/Rust/JSのコードブロック、インラインコード |
| 04_mixed_ja_en.md | 和欧混在長文、bold/italic/link装飾 |
| 05_table.md | 基本テーブル、日英混在、アラインメント |
| 06_blockquote_list.md | 引用 (ネスト含む)、リスト (ネスト含む) |
| 07_full_document.md | 全要素を含む実際的な技術文書 |

## 実行方法

```bash
# ビルド
cargo build --release

# 単一ファイルのレンダリング
md2img input.md -o output.png

# オプション
md2img input.md -o output.png \
  --width 800 \        # キャンバス幅 (pt)
  --theme catppuccin \  # テーマ名
  --ppi 144             # 出力解像度

# 全フィクスチャの一括レンダリング
md2img tests/fixtures/*.md -o output/
```

## 判断ポイントと撤退基準

- **Phase 1 完了時**: Typstの日本語組版品質がV1 (Pango) 以上であることを確認。満たさない場合はPango方式に戻る
- **Phase 2 完了時**: Markdown→Typst変換のカバレッジが実用的か判断。変換のエッジケースが多すぎる場合は、pulldown-cmarkの代わりにTypstのMarkdown対応 (議論中) を待つ選択肢もある
- **Phase 3 完了時**: 「Markdownビューア」として実用的な品質に達しているか。ここで全体の方向性を最終判断
- **Phase 4 完了時**: Kitty Graphics Protocolの帯域・レイテンシが実用的か。問題があればブロック分割戦略を再検討

## 備考

- Typstのバージョン: 0.14.x 系を想定。0.x系のためAPIが変わる可能性あり
- フォント: システムインストール前提。CI環境では Noto Sans CJK JP + JetBrains Mono をセットアップ
- ライセンス: Typstコンパイラは Apache-2.0。NOTICEファイルの再配布が必要
- `typst-as-lib` クレートを使うとWorld実装のボイラープレートが減るが、Typstバージョン追従のリスクがある。最初は自前World実装で始め、安定したら検討
