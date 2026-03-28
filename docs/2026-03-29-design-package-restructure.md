# Package Structure Reorganization

## Motivation

現状の `src/pipeline/` は「Markdown → Typst → コンパイル → PagedDocument」を担うが、
`pipeline/build.rs` が `tile.rs`, `visual_line.rs` 等のモジュール外ファイルに依存しており、
モジュール境界が曖昧になっている。また `diagram.rs`, `image.rs` がコンパイルの資材準備であるにも
かかわらず `src/` 直下に孤立している。

目的は認知的整理: モジュールの配置から責務が直感的にわかるようにする。

## Design

### Boundary

**PagedDocument** を境界とする2段階分割:

- **Stage 1 (`compile/`)**: ソース（Markdown, 画像, Mermaid）から PagedDocument を生成する
- **Stage 2 (`frame/`)**: PagedDocument の Frame ツリーを加工して視覚出力を生成する

オーケストレーションは2層:

- **`pipeline.rs`**: データフロー制御（compile/ と frame/ を橋渡し。fork 子プロセス内で実行）
- **`renderer.rs`**: プロセス制御 facade（fork/sandbox/IPC。viewer から呼ばれる公開 API）

### File Mapping

```
src/
  compile/                    # Source → PagedDocument
    markup.rs                 # ← pipeline/markup.rs
    markup_util.rs            # ← pipeline/markup_util.rs
    markup_html.rs            # ← pipeline/markup_html.rs
    content_index.rs          # ← pipeline/content_index.rs
    world.rs                  # ← pipeline/world.rs
    typst.rs                  # ← pipeline/typst_compile.rs (compile_document, dump_document)
    diagram.rs                # ← src/diagram.rs
    image.rs                  # ← src/image.rs
    mod.rs                    # new

  frame/                      # PagedDocument(Frame) → visual output
    tile.rs                   # ← src/tile.rs
    tile_cache.rs             # ← src/tile_cache.rs
    visual_line.rs            # ← src/visual_line.rs
    highlight.rs              # ← src/highlight.rs
    render_png.rs             # ← pipeline/typst_compile.rs (render_frame_to_png)
    mod.rs                    # new

  pipeline.rs                 # ← pipeline/build.rs (renamed)
  renderer.rs                 # unchanged (use path updates only)
```

### typst_compile.rs の分割

現在の `pipeline/typst_compile.rs` は2つの異なるステージの責務を持つ:

- `compile_document()`, `dump_document()` → `compile/typst.rs`
- `render_frame_to_png()` → `frame/render_png.rs`

### Public Interface

**`compile/mod.rs`**:

```rust
mod content_index;
mod diagram;
mod image;
mod markup;
mod markup_html;
mod markup_util;
mod typst;
mod world;

pub use markup::{Prescan, markdown_to_typst, prescan};
pub use typst::{compile_document, dump_document};
pub use world::{FontCache, MluxWorld};
pub use content_index::{
    BlockMapping, BoundIndex, ContentIndex, MdPosition,
    SpanKind, TextSpan, rendered_to_source_byte,
};
pub use diagram::{DiagramEntry, diagram_key, extract_diagrams, render_diagrams};
pub use image::{LoadedImages, extract_image_paths, load_images};
```

**`frame/mod.rs`**:

```rust
mod highlight;
mod render_png;
mod tile;
mod tile_cache;
mod visual_line;

pub use tile::{ContentMapping, TiledDocument, DocumentMeta};
pub use tile_cache::TilePngs;
pub use visual_line::{VisualLine, extract_visual_lines_with_map, pt_to_px};
pub use highlight::{HighlightRect, HighlightSpec};
pub use render_png::render_frame_to_png;
```

具体的な export 項目は実装時に現状コードから正確に確定する。

### Cohesion Analysis

**`compile/`** (cohesion: high)
- 共通の出力目標: PagedDocument を生成する
- 共通の入力ドメイン: ソーステキスト（Markdown, 画像パス, Mermaid 記法）
- diagram.rs, image.rs は「コンパイルに必要な資材準備」として帰属

**`frame/`** (cohesion: very high)
- 共通の入力型: typst の Frame ツリー
- 全ファイルが同じ操作パターン: Frame ツリーを走査して空間的・視覚的構造を抽出
  - tile.rs: Frame を垂直分割
  - visual_line.rs: テキストベースライン抽出
  - highlight.rs: グリフ位置特定
  - render_png.rs: Frame → ピクセル変換
  - tile_cache.rs: tile.rs の付随キャッシュ

**`pipeline.rs`** (cohesion: single responsibility)
- compile/ と frame/ を橋渡しするオーケストレーション専任

### Known Considerations

- **サイドバー生成のフィードバックループ**: `frame/visual_line.rs` の出力を使って
  サイドバー Typst を生成し、`compile/typst.rs` で2回目のコンパイルを行う。
  この越境は `pipeline.rs` が仲介する。
- **lib.rs の更新**: `mod pipeline` → `mod compile` + `mod frame` + pipeline.rs の宣言に変更。
- **use パス更新**: `crate::pipeline::*` → `crate::compile::*` / `crate::frame::*` への一括変更。
  viewer/, renderer.rs, main.rs, tests/ が影響を受ける。
- **`pipeline/` ディレクトリ削除**: 内容の移動完了後に削除。
