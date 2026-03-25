# Prescan Lifecycle Refactoring: AppContextからmarkdown依存を除去する

Previous documents:
- [`docs/2026-03-21-latin-mode-design.md`](2026-03-21-latin-mode-design.md) — Latin mode導入時の設計。prescanとAppContextの関係を初めて定義
- [`docs/2026-03-19-design-usecase-orchestration.md`](2026-03-19-design-usecase-orchestration.md) — usecase層のfork構成とBuildParams owned化
- [`docs/2026-03-18-design-security-reassessment.md`](2026-03-18-design-security-reassessment.md) — 2-stage fork設計とLandlock制約

## Problem

Latin mode設計（2026-03-21）は以下の前提を置いた:

> Prescan runs **once at startup**. The `has_cjk` result is fixed for the entire process
> lifetime — even if the file changes on disk and the viewer reloads, the CJK/latin
> determination does not change.

この「起動時に一度だけ」という判断は設計を単純化したが、AppContextにmarkdown依存の
状態（`has_cjk`）を持ち込む結果となった。

### 現在の依存チェーン

```
markdown text
  → prescan() → { image_paths, has_cjk }
    → has_cjk → theme::resolve_theme_name()
      → ResolvedTheme → AppContext
        → AppContext.build_params() → BuildParams → usecase::build_renderer()
```

AppContextの構築にmarkdownの内容が必要になっている。

### 具体的な問題

1. **AppContextがmarkdownに依存している** — `has_cjk`と`ResolvedTheme`がAppContextの
   フィールドになっている。AppContextは本来、CLI引数・config・ターミナル検出など
   「プログラム起動時に確定する状態」を保持する器であるべき

2. **prescanが2回走る** — main.rsで1回（CJK判定のためだけに）、Fork1で1回（画像URL
   抽出のため）。同じ関数を異なる目的で2箇所から呼ぶ構造

3. **viewer reloadでhas_cjkが陳腐化する** — `from_existing()`で前回の`has_cjk`を
   引き継ぐため、ファイル内容が変わってもCJK判定が更新されない。navigateで別ファイル
   を開いた場合も同様

4. **ライフサイクルの逆転** — 既存のAppContext/Sessionという器に合わせてprescanの
   タイミングが決まっている。本来はビルドパイプラインの内部処理であるprescanが
   アプリケーション初期化のクリティカルパスに昇格してしまっている

## Design: テーマ解決をビルドパイプラインに移動する

### 原則

- **AppContextはmarkdown非依存** — CLI引数、config、フォントキャッシュ、ターミナル検出
  のみ保持。起動時に一度構築し、config reload以外で変更しない
- **テーマ解決はビルドの責務** — prescanの結果（`has_cjk`）を受けてビルド時に決定。
  ドキュメントごとに異なるテーマが選ばれうる
- **prescanはビルドパイプラインに閉じる** — Landlock環境下でのみ実行。main.rsから消える

### CJK/Latinテーマ分岐について

CJK/Latinの違いはフォント選択だけでなく、テーマ全体が異なる進化をする:
- Latinテーマはitalic、letter-spacing、line-height等の調整が入る可能性が高い
- 別テーマファイルとして管理する現在のアプローチ（`catppuccin-latin.typ`）は妥当
- したがってテーマ解決は`has_cjk`を入力として必要とし続ける

### AppContext: before/after

Before:
```rust
pub struct AppContext {
    pub font_cache: &'static FontCache,
    pub config: Config,
    pub cli_overrides: CliOverrides,
    pub detected_light: bool,
    pub has_cjk: bool,              // ← markdown依存
    pub theme: ResolvedTheme,       // ← markdown依存
}
```

After:
```rust
pub struct AppContext {
    pub font_cache: &'static FontCache,
    pub config: Config,
    pub cli_overrides: CliOverrides,
    pub detected_light: bool,
}
```

`has_cjk`と`ResolvedTheme`が消える。`AppContextBuilder`から`set_has_cjk()`も消える。

### BuildParams: before/after

Before:
```rust
pub struct BuildParams {
    pub theme_name: String,          // 解決済みテーマ名
    pub theme_text: String,          // テーマのTypstソース
    pub data_files: DataFiles,       // テーマのデータファイル
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

After:
```rust
pub struct BuildParams {
    pub theme_spec: String,          // ユーザ指定のまま ("auto", "catppuccin", etc.)
    pub detected_light: bool,        // ターミナル検出結果
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

`theme_name`/`theme_text`/`data_files` → `theme_spec` + `detected_light` に置換。
テーマの解決はビルドパイプライン内部（prescan直後）で行う。

### 新しいビルドパイプライン内部フロー

```
compile_and_tile(params, images):
  // Step 1: prescan（Fork内、Landlock下で実行される）
  let prescan = prescan(&params.markdown);

  // Step 2: テーマ解決（prescanの結果を使う）
  let theme_name = resolve_theme_name(&params.theme_spec, params.detected_light, prescan.has_cjk);
  let theme_text = theme::get(theme_name)?;
  let data_files = theme::data_files(theme_name);

  // Step 3: 以降は既存のパイプライン
  markdown_to_typst(...)
  MluxWorld::new(theme_text, data_files, ...)
  typst::compile(...)
  split_into_tiles(...)
```

### main.rs: before/after

Before:
```rust
let markdown = input_source.read_all()?;
let prescan = pipeline::prescan(&markdown);               // ← main.rsでprescan
let app = AppContextBuilder::new(config, cli_overrides)
    .load_fonts()
    .set_detected_light(detected_light)
    .set_has_cjk(prescan.has_cjk)                         // ← markdown依存
    .build()?;
```

After:
```rust
let app = AppContextBuilder::new(config, cli_overrides)
    .load_fonts()
    .set_detected_light(detected_light)
    .build()?;                                             // markdownに依存しない
// markdownの読み込みはここでもいいし、各モード内でもいい
```

main.rsからprescan呼び出しが消える。AppContext構築にmarkdownが不要になるため、
markdownの読み込みタイミングも自由になる。

### usecase.rs のFork構成

2-stage fork構成（security-reassessment設計）は維持する。変更点:

```
Fork 1 (sandbox: no FS, no network):
  prescan(&markdown) → { image_paths, has_cjk }
  → image_paths を parent に返す（remote URL抽出用）
  → has_cjk は Fork 2 で再計算するため破棄でよい
    （あるいはFork 1の結果をFork 2に渡してもよい — 実装判断）

Parent (trusted):
  remote URLをfetch

Fork 2 (sandbox: FS read-only git root, no network):
  prescan(&markdown) → has_cjk を取得 [注: prescan 3回目だが、Fork間でIPC不要にするため]
  resolve_theme_name(theme_spec, detected_light, has_cjk) → テーマ決定
  local images load + remote merge
  compile_and_tile()
```

**注意**: prescanがFork 1とFork 2で2回走る点は、既存設計のsecurity-reassessment
（Fork 1とFork 2の独立性を保つ）と整合する。prescanは~1msなので性能影響は無視できる。

あるいは、Fork 1の結果（Prescan全体）をparentに返し、parentがそれをFork 2に渡す
設計も可能。Fork 2でprescanを省略できるが、IPCの複雑さとのトレードオフ。

### Viewer config reload

Before:
```rust
// viewer/mod.rs — config reload時
let resolved = theme::resolve_theme_name(&new_config.theme, app.detected_light, app.has_cjk);
if theme::get(resolved).is_none() {
    // error: unknown theme
}
```

After:
```rust
// config reload時のテーマ検証
if !theme::is_valid_theme_spec(&new_config.theme) {
    // error: unknown theme spec
}
// "auto" は常に有効（ビルド時に解決される）
// 明示テーマ名は theme::get(name).is_some() で静的チェック
// has_cjkは不要 — latin variant選択はビルド時の責務
```

`is_valid_theme_spec()`は新設のヘルパー。`"auto"`/`"dark"`/`"light"`はエイリアスとして
常に有効、明示テーマ名はレジストリに存在するかチェック。`has_cjk`に依存しない。

### Viewer reload / navigate

ドキュメント更新やファイルナビゲーションのたびにビルドパイプラインが走り、その中で
prescanが実行される。テーマ解決はビルドのたびに行われるため、CJK判定は常に最新の
markdownを反映する。特別な処理は不要 — 自然にそうなる。

## Migration path

1. **BuildParams変更** — `theme_name`/`theme_text`/`data_files` → `theme_spec` + `detected_light`
2. **compile_and_tile / compile_content内部** — prescan結果からテーマ解決を行う
3. **AppContext変更** — `has_cjk`と`theme`フィールドを除去、`build_params()`を更新
4. **AppContextBuilder変更** — `set_has_cjk()`除去、`build()`からテーマ解決を除去
5. **main.rs変更** — prescan呼び出しを除去、markdown読み込みのタイミングを自由化
6. **viewer config reload** — `is_valid_theme_spec()`による静的検証に変更
7. **テスト更新** — AppContextBuilderのテストからhas_cjk/theme関連を除去

## Not in scope

- Fork 1 / Fork 2の統合（2-stage fork構成は維持）
- prescanの検出ロジック拡張（CJK判定アルゴリズムの変更等）
- テーマファイル自体の変更
- viewer内部のイベントループ構造
