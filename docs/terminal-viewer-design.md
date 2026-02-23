# tmark ターミナルビューア設計

Phase 4（Kitty Graphics Protocol 表示）および Phase 5（Vimライクナビゲーション）の設計文書。
セッション内の議論で確定した方針と、残課題を記録する。

## 設計コンテキスト

### 解決済みの技術的懸念

| 懸念 | 解決策 | 確認方法 |
|---|---|---|
| 無限縦長ページ | `#set page(height: auto)` で1ページに全コンテンツ | catppuccin.typ で使用中 |
| グリフ・行座標の取得 | `Frame::items()` → `(Point, FrameItem)` | `--dump` で実機確認済み |
| ブロック境界の検出 | トップレベル `Group` がブロック要素に対応 | `--dump` で実機確認済み |
| 画像キャッシュ+クロップ | Kitty の `i=` でキャッシュ、`x,y,w,h` でクロップ | プロトコル仕様で確認 |
| テキストと画像の共存 | 別セルに配置すれば干渉しない | プロトコル仕様で確認 |
| スクロール性能 | `a=p` で `y` を変えるだけ。画像再送信不要 | プロトコル仕様で確認 |

### 確定した基本方針

1. **`height: auto` の1枚画像方式を採用**
   - Typst に `height: auto` を指定し、全コンテンツを1ページにレンダリング
   - 1枚の PNG をターミナルに送信し、ビューポートクロップで表示
   - ページ分割方式（`height: 固定値`）は不採用
     - 理由: ページつなぎ目で両ページのクロップが必要になり、マージンの重複も発生する
     - `height: auto` + ビューポートクロップの方が単純で、つなぎ目問題が存在しない

2. **Kitty Graphics Protocol のソース矩形クロップを活用**
   - 画像全体を1回送信（`a=t, i=ID, f=100` — Ghostty では `a=T` 不可、詳細は下記）
   - 表示は `a=p, i=ID, x=0, y=scroll_offset, w=W, h=H` で制御
   - スクロールは `y` パラメータの変更のみ

3. **テキスト+画像の混合レイアウト**
   - 左マージン: 行番号 + 選択マーカー（ANSI テキスト）
   - 中央: レンダリング画像（Kitty 画像配置）
   - 下部: ステータスバー（ANSI テキスト）
   - カーソル移動は `\x1b[row;colH` で絶対座標指定（`\n` はスクロールリスクがあるため不使用）

## 描画パイプライン

```
[Markdown入力]
     ↓
[pulldown-cmark → convert.rs → Typst記法]
     ↓
[TmarkWorld → typst::compile → PagedDocument]
     ↓
[typst_render::render → tiny_skia::Pixmap → PNG bytes]
     ↓
[base64エンコード → チャンク分割 → Kitty送信 (a=t, i=ID)]
     ↓
[ビューポート配置 (a=p, i=ID, y=offset, c=cols, r=rows)]
     ↓
[行番号+ステータスバー (ANSI テキスト)]
```

## Prefetch アーキテクチャ

### 概要

スクロール時の次ストリップ遷移でキャッシュミスが発生すると、`typst_render::render()` + PNG encode の同期レンダリングで UI がブロックされる。これを解消するため、バックグラウンドスレッドで隣接ストリップを先行レンダリングし、スクロール時にキャッシュヒットさせる。

**使用技術**: `std::thread::scope` + `std::sync::mpsc` チャンネルのみ。`Arc`・`Mutex` 不要。

### StripDocument / StripDocumentCache 分割

`thread::scope` 内で worker が `&strip_doc` を借用する場合、main thread は `&mut self` メソッドを呼べない（`&` と `&mut` の共存は borrow checker が拒否）。

**解決策**: キャッシュを `StripDocument` から分離し、`StripDocument` を完全 immutable（全メソッド `&self`）にする。

- `StripDocument`: ストリップ Frame 群 + メタデータ。`render_strip(&self, idx) -> Vec<u8>` で純関数的にレンダリング。
- `StripDocumentCache`: `HashMap<usize, Vec<u8>>` ベースのキャッシュ。main thread 専用（`&mut`）。

```
worker thread: &StripDocument  ──┐
                                 ├─ &T は Copy、共存可能
main thread:   &StripDocument  ──┘
               &mut StripDocumentCache  ── main thread 専用、競合なし
```

### スレッド間データフロー

```
                    mpsc::channel::<usize>        (prefetch request)
Main thread ─────────────────────────────────→ Worker thread
             ←───────────────────────────────
                mpsc::channel::<(usize, Vec<u8>)> (rendered PNG)

Main thread:                          Worker thread:
  redraw → cache.get_or_render()        req_rx.recv() (ブロック待ち)
  send_prefetch → req_tx.send(idx)      drain-to-latest (最新のみ処理)
  res_rx.try_recv() → cache.insert()    doc.render_strip(idx) → res_tx.send()
```

Worker は drain-to-latest パターンを使用: 急速スクロール時はキュー内の古いリクエストをスキップし、最新のインデックスのみレンダリングする。

### イベントループ構造

```
'outer: loop {  ← rebuild loop (初回 + resize)
    strip_doc = build_strip_document(...)
    cache = StripDocumentCache::new()

    thread::scope(|s| {
        s.spawn(worker)  ← prefetch worker

        loop {  ← inner event loop
            res_rx.try_recv() → cache.insert()  // drain prefetch results
            event::poll() → handle key/resize
            if dirty → redraw() + send_prefetch()
        }
        // req_tx drop → worker の recv() が Err → worker 終了
    })  // ← scope が worker の join を待つ

    match exit {
        Quit → break,
        Resize → delete images, update layout, continue 'outer
    }
}
```

### Resize 時の Worker ライフサイクル

1. main thread が `ExitReason::Resize` を return → closure 終了
2. `req_tx` が drop される
3. scope が worker の join を待つ
4. worker: レンダリング中なら完了まで待つ（`&strip_doc` はまだ生存）→ `recv()` が `Err` → 終了
5. scope 完了 → outer loop continue → 新 `strip_doc` + 新 scope + 新 worker

レイテンシ: worker がレンダリング中の場合、scope join で最大 ~200ms 待つ（1ストリップ分）。実用上問題なし。

### スレッド安全性

- `Frame.items` は `Arc<LazyHash<Vec<...>>>` — clone は refcount bump のみ
- `typst_render::render(&Page, f32)` — ステートレス、ローカル Pixmap に描画
- `Content::empty()` — `RawContent` は `unsafe impl Send + Sync`
- → `StripDocument` は `Send + Sync`（コンパイラが検証）

## ビューポートとスクロール

### 座標変換

```
Typst座標 (pt) → ピクセル座標 (px)
  pixel = pt × (ppi / 72.0)

例: ppi=144, pixel_per_pt=2.0
  (40pt, 103.3pt) → (80px, 206.6px)
```

### ビューポート計算

```
terminal_rows     = ターミナルの行数
terminal_cols     = ターミナルの列数
left_margin_cols  = 行番号表示に必要な列数（例: 6）
status_rows       = ステータスバーの行数（例: 1）

image_cols = terminal_cols - left_margin_cols
image_rows = terminal_rows - status_rows

# ターミナルのピクセルサイズ（ioctl TIOCGWINSZ で取得）
cell_width_px  = terminal_pixel_width / terminal_cols
cell_height_px = terminal_pixel_height / terminal_rows

viewport_width_px  = image_cols × cell_width_px
viewport_height_px = image_rows × cell_height_px
```

### 行境界スナップ

行の途中で画像が切れるのを防ぐため、スクロール位置を行境界にスナップする。

**手順:**
1. `PagedDocument` のフレームツリーを走査
2. 全 `TextItem` の絶対Y座標を収集（再帰ウォークで `Group` のオフセットを加算）
3. Y座標でソート・重複除去 → 行境界リスト
4. スクロール要求時、要求Y位置に最も近い行境界にスナップ
5. スナップされたY座標をピクセルに変換 → `a=p` の `y` パラメータに使用

**行境界データの構造:**

```rust
struct LineInfo {
    y_pt: f64,         // 行のY座標（pt）
    y_px: u32,         // 行のY座標（px）
    height_pt: f64,    // 行の高さ（pt）
    // 将来: block_type, markdown_line_number, ...
}
```

## 行番号のマッピング

### 課題

ターミナルの「行」（画像の行方向の位置）と、Markdownソースの行番号は1:1に対応しない。
- Typstのレイアウトで折り返しが発生する
- 見出し・コードブロック等は複数のTypst行を占有する
- Markdownの空行はTypstでは段落間スペースになる

### 方針

**Markdown行番号ではなく、ビューポート内の「視覚的行番号」を表示する。**

フレームツリーの `TextItem` Y座標から視覚行を算出:
1. 同一Y座標の `TextItem` をグルーピング → 1視覚行
2. ビューポート内に含まれる視覚行を抽出
3. 各視覚行にインデックスを付与して表示

将来的に、`Glyph::span` フィールドからTypstソース上の位置を逆引きし、
さらにMarkdownソースの行番号に対応付けることも可能。

## ターミナルサイズ検出

```rust
// ioctl TIOCGWINSZ で取得
struct WinSize {
    ws_row: u16,    // 行数
    ws_col: u16,    // 列数
    ws_xpixel: u16, // ウィンドウ幅（ピクセル）
    ws_ypixel: u16, // ウィンドウ高さ（ピクセル）
}
```

- `ws_xpixel / ws_col` でセルあたりのピクセル幅を算出
- `ws_ypixel / ws_row` でセルあたりのピクセル高さを算出
- Kitty の `c,r` パラメータにセル数を、Typst の `width` にピクセル幅を使用

**ターミナルリサイズ時** (`SIGWINCH`):
1. 新しいサイズを再取得
2. 必要ならTypstの `width` を変えて再コンパイル+再レンダリング
3. 新しいPNGを再送信
4. ビューポートを再計算

## 入力処理（Phase 5 構想）

`crossterm` による raw モードでのキー入力捕捉。

| キー | 動作 |
|---|---|
| `j` / `↓` | 1行下にスクロール（行境界スナップ） |
| `k` / `↑` | 1行上にスクロール |
| `d` / `Page Down` | 半画面下にスクロール |
| `u` / `Page Up` | 半画面上にスクロール |
| `g` | 先頭へ |
| `G` | 末尾へ |
| `q` | 終了 |
| `/` | インクリメンタルサーチ（`TextItem::text` から全文検索） |
| `v` + 移動 + `y` | 範囲選択 → OSC 52 でクリップボード送信 |

## 残課題と将来の改良

### Ghostty 固有の制約（spike_kitty で検証済み）

spike_kitty (`src/bin/spike_kitty.rs`) での実機検証で判明した Ghostty の挙動:

1. **`\x1b[2J` 禁止（唯一の地雷）**: 画像データごと消える。配置の削除は `a=d,d=i,i=ID,q=1`（小文字 `i` = データ保持）を使う。
2. **`a=T` / `a=t` どちらもOK**: `a=T` でも `i=` 付きならキャッシュされ `a=p` で参照可能。
3. **スクロール手順**: `a=d,d=i` → `a=p` の繰り返し。画像再送信は不要。

詳細は `docs/kitty-graphics-protocol.md` の「Ghostty 固有の注意点」セクションを参照。

### ファーストビュー最適化

ストリップベースレンダリング + prefetch で解決済み:
- ドキュメントは `height: auto` で1ページにコンパイル後、Frame ツリーを垂直ストリップに分割
- ビューポート内のストリップのみオンデマンドで PNG レンダリング
- 隣接ストリップをバックグラウンドスレッドで先行レンダリング
- ピークメモリはストリップサイズに比例（ドキュメント全体ではない）

### 転送方式の最適化

| 方式 | メリット | デメリット |
|---|---|---|
| `t=d` (直接) | SSH等リモートでも動作 | base64オーバーヘッド(33%増) |
| `t=t` (一時ファイル) | ゼロコピーに近い | ローカルのみ、ファイルI/O |
| `t=s` (共有メモリ) | 最速 | POSIX依存、実装が複雑 |

初期実装は `t=d`（最もポータブル）。パフォーマンス問題が出たら `t=t` を検討。

### Kitty非対応ターミナルへのフォールバック

- APC は未対応ターミナルでも安全に無視される
- フォールバック: 画像なしのテキストビューア（Markdownをそのまま表示）
- iTerm2: 独自のインライン画像プロトコル（OSC 1337）への対応は将来検討

### テーマとページ幅の連携

ターミナル幅からTypstの `width` を動的計算する際、テーマの `margin` を考慮する必要がある。

```
typst_width_pt = (image_cols × cell_width_px) / pixel_per_pt
```

テーマの `#set page(width: ...)` はCLI側で上書きされる（`world.rs` で `#set page(width: {width}pt)` を挿入）。
