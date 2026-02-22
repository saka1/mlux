# Typst 0.14.2 API メモ

tmark（Markdown→Typst→PNG パイプライン）実装時の知見。
将来の API バージョンアップ時の参照用。

## 公開版 vs 開発版 API

`typst/` ディレクトリのソースは crates.io の 0.14.2 より新しい開発版で、API が異なる。
常に公開版（crates.io）の API を参照すること。

主な差異:
- **typst-kit features**: 公開版は `embed-fonts`, `fonts`。開発版は `embedded-fonts`, `scan-fonts`
- **typst-kit フォント API**: 公開版は `FontSearcher`/`FontSlot`/`Fonts` パターン。開発版は `FontStore`/`FontSource` トレイト
- **FileId**: 公開版は `FileId::new(Option<PackageSpec>, VirtualPath)`。開発版は `RootedPath::new(VirtualRoot, VirtualPath).intern()`
- **VirtualPath**: 公開版の `VirtualPath::new(path)` は `VirtualPath` を直接返す。開発版は `Result<VirtualPath, PathError>`
- **World::today**: 公開版は `Option<i64>`。開発版は `Option<Duration>`

## World トレイト (`typst::World`)

```rust
#[comemo::track]
pub trait World: Send + Sync {
    fn library(&self) -> &LazyHash<Library>;
    fn book(&self) -> &LazyHash<FontBook>;
    fn main(&self) -> FileId;
    fn source(&self, id: FileId) -> FileResult<Source>;
    fn file(&self, id: FileId) -> FileResult<Bytes>;
    fn font(&self, index: usize) -> Option<Font>;
    fn today(&self, offset: Option<i64>) -> Option<Datetime>;
}
```

- `Send + Sync` が必要
- `Source`, `Bytes`, `Font` はすべて参照カウント型でクローンが軽量
- `#[comemo::track]` によりインクリメンタルコンパイルの追跡が有効になる

## FileId / Source / Bytes の生成

```rust
// 仮想ファイル ID を作成（パッケージなし、プロジェクトルート相対パス）
let vpath = VirtualPath::new("main.typ");
let id = FileId::new(None, vpath);

// ID とテキストから Source を作成
let source = Source::new(id, text_string);

// Source から Bytes を作成（World::file 用）
let bytes = Bytes::from_string(source.clone());

// 生バイトデータから Bytes を作成
let bytes = Bytes::new(vec_of_bytes);
```

## FontSearcher（typst-kit 0.14.2）

```rust
use typst_kit::fonts::{FontSearcher, FontSlot, Fonts};

let Fonts { book, fonts } = FontSearcher::new()
    .include_system_fonts(true)
    .search();

// World 実装での使い方:
// fn book() -> &LazyHash<FontBook> { &LazyHash::new(book) }
// fn font(index) -> Option<Font> { fonts.get(index)?.get() }
```

## Library の構築

```rust
use typst::{Library, LibraryExt};

let library = Library::default();  // 標準ライブラリ
// カスタマイズする場合:
let library = Library::builder()
    .with_inputs(dict)
    .with_features(features)
    .build();
```

## コンパイル

```rust
use typst::layout::PagedDocument;

let warned = typst::compile::<PagedDocument>(&world);
// warned.output: Result<PagedDocument, EcoVec<SourceDiagnostic>>
// warned.warnings: EcoVec<SourceDiagnostic>

let document = warned.output?;
// document.pages: Vec<Page>
```

## レンダリング

```rust
let pixel_per_pt = ppi / 72.0;  // 例: 144/72 = 2.0（2倍解像度）
let pixmap = typst_render::render(&document.pages[0], pixel_per_pt);
let png_bytes = pixmap.encode_png()?;
```

## FontBook

- フォントファミリ名は内部の BTreeMap で**小文字化**して格納される
- `book.contains_family("ipagothic")`（小文字）でチェック
- `book.families()` で全ファミリと FontInfo をイテレーション可能

## #include vs #import

公開版 0.14.2 では、仮想ファイルシステム上で別 FileId を持つファイルに対して
`#include` を使った場合、`#set` ルールが後続コンテンツに伝播しなかった。
回避策として、テーマの内容を `main.typ` に直接インライン展開する:

```
// 動かなかったパターン:
//   main.typ: #include "theme.typ" \n #include "content.typ"
// 動くパターン:
//   main.typ: <テーマの set ルール> \n <コンテンツ>
```

インライン方式のほうがシンプルで、マルチファイル仮想 FS の問題を回避できる。

## 既知の注意点

- `typst_render::render()` は `tiny_skia::Pixmap` を返し、その `encode_png()` は `Result<Vec<u8>, EncodingError>` を返す
- フォントフォールバックチェーン内の未インストールフォントに対して Typst が警告を出すが、チェーン内に1つでもフォントが見つかれば無害
- `#set page()` の `height: auto` でページがコンテンツに合わせて自動サイズになる（可変高さの単一ページを生成）
