# ビューア状態モデル

ターミナルビューア (`src/viewer/`) の実行時状態は、ビューアの概念的な階層を反映する
3 つのドメイン構造体に整理されている:

```
Session (プロセス寿命)
  └── Viewport (ドキュメントビルド単位)
        └── ViewContext (エフェクト適用単位、読取専用)
```

## 概要

| 構造体 | 生存期間 | 可変性 | 配置 |
|--------|---------|--------|------|
| `Session` | プロセス開始 → 終了 | `&mut self` | `mod.rs` 外部ループ |
| `Viewport` | 1 回のドキュメントビルド | `&mut self` | `mod.rs` 内部ループ |
| `ViewContext` | 1 回のエフェクト適用サイクル | 読取専用 (`&`) | キーイベントごとに生成 |

## Viewport

**ドキュメントビルドに対するユーザの対話的な視界。**

`Viewport` はドキュメントがコンパイルされるたびに生成される（初回読込・リサイズ・
リロード・ナビゲーション）。内部イベントループが終了し、新しいビルドが開始されると破棄される。

```rust
pub(super) struct Viewport {
    pub mode: ViewerMode,           // Normal / Search / Command / UrlPicker
    pub view: ViewState,            // スクロール位置 + ビューポート寸法
    pub tiles: LoadedTiles,         // タイルレンダリングキャッシュ（端末側画像）
    pub flash: Option<String>,      // 一時的なステータスメッセージ ("Yanked L56")
    pub dirty: bool,                // 次フレームで再描画が必要
    pub last_search: Option<LastSearch>,  // n/N ナビゲーション用の検索結果
}
```

**各フィールドがここに属する理由:**
- `mode` — ビューポートの表示内容を決定する（タイル表示 / 検索ピッカー / コマンドプロンプト）
- `view` — スクロール位置とジオメトリ。ドキュメントに対するビューポートの「窓」
- `tiles` — レンダリングキャッシュ。端末にアップロード済みのタイル群
- `flash`, `dirty` — ビルドごとにリセットされる一時的な UI 状態
- `last_search` — 1 回のビルド内でモード遷移をまたいで持続（n/N ナビゲーション用）

**主要メソッド:**
```rust
impl Viewport {
    fn apply(&mut self, effect: Effect, ctx: &ViewContext) -> Result<Option<ExitReason>>
}
```

## ViewContext

**エフェクト適用のための読取専用環境。**

キーイベントごとに生成される一時的な参照バンドル。`Viewport::apply` が参照するが
変更してはならないデータをまとめる。

```rust
pub(super) struct ViewContext<'a> {
    pub layout: &'a Layout,         // 端末セル/ピクセルジオメトリ
    pub acc_value: Option<u32>,     // 現在の数値プレフィクス（スナップショット）
    pub input: &'a InputSource,     // ファイルパスまたは stdin
    pub jump_stack: &'a [JumpEntry], // ナビゲーション履歴（GoBack 判定用）
    pub markdown: &'a str,          // ソースドキュメントテキスト
    pub visual_lines: &'a [VisualLine], // ドキュメント構造
}
```

**`acc` が参照ではなくスナップショット値である理由:**
- `InputAccumulator` は入力処理中に `map_key_event(&mut acc)` で変更される
- `Viewport::apply` は現在の蓄積値 (`.peek()`) だけを必要とする
- `Option<u32>` で保持すれば、エフェクトループをまたぐボローが不要になる
- アキュムレータは入力処理が行われる `mod.rs` に留めるべき

## Session

**ビューア起動から終了までの永続的なセッション。**

ドキュメント再ビルド（リサイズ・リロード・ファイルナビゲーション）をまたいで持続する。
設定・ファイル管理・ビルド間状態を含む。

```rust
pub(super) struct Session {
    pub layout: Layout,             // 端末レイアウト（リサイズで再計算）
    pub config: Config,             // アプリケーション設定
    pub cli_overrides: CliOverrides, // config reload 時に保持される CLI 引数
    pub input: InputSource,         // 現在の入力ソース（ファイル/stdin）
    pub filename: String,           // ステータスバー用の表示名
    pub watcher: Option<FileWatcher>, // ファイル変更監視
    pub jump_stack: Vec<JumpEntry>, // タグジャンプ履歴（ブラウザの戻るボタン相当）
    pub scroll_carry: u32,          // リビルド後に復元するスクロール位置
    pub pending_flash: Option<String>, // リビルド後に表示するメッセージ
    pub watch: bool,                // ファイル監視の有効/無効
}
```

**`cli_overrides` を Session に含める理由:**
- `watch` と同様、不変のセッションレベル設定である
- config reload (`handle_exit` の `ExitReason::ConfigReload`) でのみ使用
- Session に含めることで `handle_exit` の引数が 3 つに削減: `&mut self`, `exit`, `scroll_position`

**主要メソッド:**
```rust
impl Session {
    fn handle_exit(&mut self, exit: ExitReason, scroll_position: u32) -> Result<bool>
}
```

## データフロー

### イベント処理（内部ループ）

```
キーイベント
  → map_key_event / map_search_key / ...  (input.rs)
  → モードハンドラが Vec<Effect> を返す    (mode_normal.rs, mode_search.rs, ...)
  → 各 effect について:
      ViewContext を生成（session + ドキュメント参照のスナップショット）
      Viewport::apply(effect, &ctx)        (effect.rs)
        → ビューポート状態を変更
        → ExitReason を返す場合あり
```

### 終了理由の処理（外部ループ）

```
内部ループからの ExitReason
  → Session::handle_exit(exit, viewport.view.y_offset)  (effect.rs)
    → セッション状態を変更（config, input, layout, ...）
    → true を返すとビューア終了
  → 終了でなければ: ドキュメント再ビルド → 新しい Viewport を生成 → 内部ループ再突入
```

### 不連続フィールドボロー

Viewport 構造体はモードディスパッチにおいて Rust の不連続フィールドボローを活用する:

```rust
let effects = match &mut vp.mode {        // vp.mode をボロー
    ViewerMode::Normal => {
        vp.flash = None;                   // vp.flash をボロー（別フィールド ✓）
        let mut ctx = NormalCtx {
            state: &vp.view,               // vp.view をボロー（別フィールド ✓）
            last_search: &mut vp.last_search, // 別フィールド ✓
            ...
        };
        mode_normal::handle(action, &mut ctx)  // 所有値 Vec<Effect> を返す
    }
    ...
};
// match 終了 → vp.mode のボローが解放される
// effects は所有データなので vp.mode への参照を持たない
vp.apply(effect, &ctx)?;                  // &mut self で vp 全体をボロー（OK ✓）
```

これが安全な理由:
1. Rust の NLL ボローチェッカーは構造体の異なるフィールドへの同時ボローを許可する
2. `Vec<Effect>` は所有データであり、`vp.mode` への参照を保持しない
3. match 終了後、`vp.mode` のボローは `vp.apply()` が `&mut self` を取る前に解放される
