# Usecase Orchestration Layer Refactoring

## Problem

ドキュメントビルドの実行順序を制御する「ユースケース層」の責務が散逸している。

1. **child closureの重複** — `fork_renderer()` と `fork_dump()` で sandbox適用 → local image load → remote merge → FontCache::new() → BuildParams再構築 が繰り返されている
2. **呼び出し側の重複** — `main.rs` (render) と `viewer/mod.rs` で `prepare_images()` → `fork_renderer()`/`spawn_renderer()` のシーケンスと引数受け渡しが重複
3. **責務の混在** — `fork_render/mod.rs` が「sandboxed fork実行機構」と「ドキュメントビルドのオーケストレーション」の2責務を持つ
4. **BuildParamsの借用問題** — `BuildParams<'a>` がfork境界を越えられず、全フィールドの手動clone + 再構築ボイラープレートが発生

## Design

### Module structure

```
src/
  fork_sandbox/           (renamed from fork_render/)
    mod.rs                fork_compute() のみ残る
    process.rs            TypedWriter/Reader, fork_with_channels, ChildProcess (変更なし)
    sandbox.rs            Landlock enforcement (変更なし)

  usecase.rs              (新設) ドキュメントビルドのオーケストレーション
                          - build_renderer()
                          - build_renderer_blocking()
                          - build_dump()
                          - TileRenderer, Request, Response 等のレンダリング固有型

  pipeline/build.rs       compile_and_tile(), compile_content() (変更なし)
```

### Responsibility separation

| Module | 責務 | 知っていること |
|--------|------|----------------|
| `fork_sandbox/` | sandboxed fork実行機構 | fork, IPC, Landlock |
| `usecase.rs` | ドキュメントビルドの実行順序 | image準備、compile、tile rendering |
| `pipeline/` | コアロジック | Markdown→Typst→compile→tile |

`fork_sandbox/` はドキュメントやレンダリングを知らない。
`pipeline/` はfork/sandboxを知らない。
`usecase.rs` が両者を組み合わせてユースケースを実現する。

### BuildParams owned化

Before:
```rust
pub struct BuildParams<'a> {
    pub theme_name: &'a str,
    pub theme_text: &'a str,
    pub data_files: crate::theme::DataFiles,
    pub markdown: &'a str,
    pub base_dir: Option<&'a Path>,
    pub width_pt: f64,
    pub sidebar_width_pt: f64,
    pub tile_height_pt: f64,
    pub ppi: f32,
    pub fonts: &'a FontCache,
    pub allow_remote_images: bool,
}
```

After:
```rust
#[derive(Clone)]
pub struct BuildParams {
    pub theme_name: String,
    pub theme_text: String,
    pub data_files: crate::theme::DataFiles,
    pub markdown: String,
    pub base_dir: Option<PathBuf>,
    pub width_pt: f64,
    pub sidebar_width_pt: f64,
    pub tile_height_pt: f64,
    pub ppi: f32,
    pub fonts: &'static FontCache,
    pub allow_remote_images: bool,
}
```

- `&'a str` → `String`, `&'a Path` → `PathBuf` (軽量、clone可能)
- `&'a FontCache` → `&'static FontCache` (parent側で `Box::leak`)
- ライフタイムパラメータ消滅 → fork境界を `.clone()` で越えられる
- `DataFiles` は `Copy` なのでそのまま

FontCacheのleak: `AppContextBuilder::build()` 内で `Box::leak(Box::new(FontCache::new()))` → `&'static FontCache`。
fork後、child側ではCOWでメモリが引き継がれるためそのまま参照有効。
childで `FontCache::new()` を再実行する必要がなくなる（性能改善）。

### usecase.rs public API

```rust
/// ドキュメントをビルドし、tile-on-demand rendererを返す。
/// viewerモード用: wait_for_metaは呼び出し側が行う。
pub fn build_renderer(
    params: BuildParams,
    no_sandbox: bool,
) -> Result<(TileRenderer, ChildProcess)>

/// build_renderer + wait_for_meta を同期的に行う。
/// renderモード用（loading UIが不要）。
pub fn build_renderer_blocking(
    params: BuildParams,
    no_sandbox: bool,
) -> Result<(DocumentMeta, TileRenderer, ChildProcess)>

/// ドキュメントをビルドし、Typstソース+frame treeをstderrにdumpして終了。
pub fn build_dump(
    params: BuildParams,
    no_sandbox: bool,
) -> Result<ChildProcess>
```

### Internal flow (build_renderer)

```
1. fork_compute(sandbox=no_fs) { extract_image_paths(&params.markdown) }
2. parent: fetch remote images (load_images, remote only)
3. fork_with_channels(child_fn):
     child:
       enforce_sandbox(read_only base_dir)
       child_setup(&params, remote_images)  // local images + merge + compile_and_tile
       send Meta
       enter request loop
   parent:
     return (TileRenderer, ChildProcess)
```

`child_setup` は `usecase.rs` 内のprivate関数:

```rust
fn child_setup(
    params: &BuildParams,
    remote_images: LoadedImages,
) -> Result<TiledDocument> {
    let image_paths = extract_image_paths(&params.markdown);
    let (mut images, errors) = image::load_images(&image_paths, params.base_dir.as_deref(), false);
    for err in &errors {
        log::warn!("{err}");
    }
    images.extend(remote_images);
    compile_and_tile(params, images)
}
```

Note: child側でもextract_image_pathsを再実行する（Fork 1の結果をIPCで渡すより単純）。
Fork 1の目的はremote URL抽出のためのsandboxed parseであり、child側のlocal image loadには
パスの再抽出で十分。

### Internal flow (build_dump)

`build_renderer` と同じ2-stage forkパターンだが、child側で `compile_and_dump` を呼び、
stderrに出力して終了する（request loopなし、MetaのIPC送信もなし）。

```
1. fork_compute(sandbox=no_fs) { extract_image_paths(&params.markdown) }
2. parent: fetch remote images
3. fork_with_channels(child_fn):
     child:
       enforce_sandbox(read_only base_dir)
       child_setup variant: local images + merge + compile_and_dump → stderr
       exit
   parent:
     return ChildProcess (caller waits for exit)
```

### Calling site changes

```rust
// main.rs (render mode) — before
let params = app.build_params(&markdown, read_base.as_deref(), ...);
let (image_paths, remote_images) = fork_render::prepare_images(&markdown, allow_remote, no_sandbox)?;
let (meta, renderer, child) = fork_render::spawn_renderer(&params, &image_paths, remote_images, read_base, no_sandbox)?;

// main.rs (render mode) — after
let params = app.build_params(markdown, read_base, ...);
let (meta, renderer, child) = usecase::build_renderer_blocking(params, no_sandbox)?;
```

```rust
// viewer/mod.rs — before
let params = app.build_params(&markdown, base_dir, ...);
let (image_paths, remote_images) = fork_render::prepare_images(&markdown, allow_remote, no_sandbox)?;
let (renderer, child) = fork_render::fork_renderer(&params, &image_paths, remote_images, read_base, no_sandbox)?;

// viewer/mod.rs — after
let params = app.build_params(markdown, base_dir, ...);
let (renderer, child) = usecase::build_renderer(params, no_sandbox)?;
```

### fork_sandbox module (after cleanup)

`fork_sandbox/mod.rs` に残るpublic API:

```rust
/// Fork a sandboxed child that computes a value and returns it.
pub fn fork_compute<T, F>(sandbox_read_base: Option<&Path>, no_sandbox: bool, f: F) -> Result<T>
where T: Serialize + DeserializeOwned, F: FnOnce() -> T;
```

加えて `process` サブモジュールを `pub(crate)` で公開し、`usecase.rs` が
`fork_with_channels`, `TypedWriter`, `TypedReader`, `ChildProcess` を直接使えるようにする。

```rust
pub(crate) mod process;  // usecase.rs から利用
mod sandbox;              // fork_compute内部でのみ使用
```

sandbox enforcement は `fork_compute` 経由でも、`usecase.rs` のchild closure内で
直接 `sandbox::enforce_sandbox()` を呼ぶ形でも利用できるようにする:

```rust
pub(crate) mod sandbox;  // or: pub(crate) fn enforce_sandbox を mod.rs で re-export
```

### MluxWorld lifetime changes

`MluxWorld<'f>` の `'f` は `FontCache` の借用に由来する。
`FontCache` が `&'static` になると `'f = 'static` となり、ライフタイムパラメータを
除去できる可能性がある。ただし他の借用（source text等）があれば残る。
実装時に判断する（設計上のブロッカーではない）。

## Not in scope

- `pipeline/` 内部のリファクタリング（compile_content, compile_and_tile等の構造）
- viewer内部のイベントループ構造
- fork_sandbox の過剰な汎用化（fork_service的な汎用request loop抽象は作らない）
- テスト用の `build_tiled_document()` — 引き続き `pipeline/build.rs` に残る（sandbox不要のテスト向けパス）
