# mlux ターミナルビューア設計

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

スクロール時の次タイル遷移でキャッシュミスが発生すると、`typst_render::render()` + PNG encode の同期レンダリングで UI がブロックされる。これを解消するため、バックグラウンドスレッドで隣接タイルを先行レンダリングし、スクロール時にキャッシュヒットさせる。

**使用技術**: `std::thread::scope` + `std::sync::mpsc` チャンネルのみ。`Arc`・`Mutex` 不要。

### TiledDocument / TiledDocumentCache 分割

`thread::scope` 内で worker が `&tiled_doc` を借用する場合、main thread は `&mut self` メソッドを呼べない（`&` と `&mut` の共存は borrow checker が拒否）。

**解決策**: キャッシュを `TiledDocument` から分離し、`TiledDocument` を完全 immutable（全メソッド `&self`）にする。

- `TiledDocument`: コンテンツ+サイドバーの Frame 群 + メタデータ。`render_tile()` / `render_sidebar_tile()` で純関数的にレンダリング。
- `TiledDocumentCache`: `HashMap<usize, TilePngs>` ベースのキャッシュ（`TilePngs` = content + sidebar PNG のペア）。main thread 専用（`&mut`）。

```
worker thread: &TiledDocument  ──┐
                                 ├─ &T は Copy、共存可能
main thread:   &TiledDocument  ──┘
               &mut TiledDocumentCache  ── main thread 専用、競合なし
```

### スレッド間データフロー

```
                    mpsc::channel::<usize>             (prefetch request)
Main thread ─────────────────────────────────→ Worker thread
             ←───────────────────────────────
                mpsc::channel::<(usize, TilePngs)>    (rendered content+sidebar PNG pair)

Main thread:                          Worker thread:
  redraw → cache.get_or_render()        req_rx.recv() (ブロック待ち)
  send_prefetch → req_tx.send(idx)      FIFO 順に各リクエストを処理
  res_rx.try_recv() → cache.insert()    render_tile(idx) + render_sidebar_tile(idx) → res_tx.send()
```

Worker は FIFO 順に各リクエストを処理する。`send_prefetch()` は `[current+1, current+2, current-1]` の独立した複数リクエストを送るため、drain-to-latest（最後だけ処理）だと手前のタイルがプリフェッチされず、メインスレッドで同期レンダリングが発生する。

### in_flight による二重レンダリング防止

`cache.contains()` だけでは TOCTOU (Time-of-Check-to-Time-of-Use) が発生する:

```
時刻  Worker thread                    Main thread
 T1   render_tile(2) 完了
      res_tx.send((2, png))
 T2                                    send_prefetch():
                                         cache.contains(2) → false  ← 結果はチャネル内、未 drain
                                         req_tx.send(2)             ← 二重リクエスト！
 T3                                    res_rx.try_recv() → cache.insert(2)
 T4   render_tile(2) 再実行           ← 無駄なレンダリング
```

**解決策**: `HashSet<usize>` の `in_flight` で「送信済み・未受信」の index を追跡する。

- `send_prefetch()`: `cache.contains()` **と** `in_flight.contains()` の両方を検査
- `send_prefetch()`: リクエスト送信時に `in_flight.insert(idx)`
- `res_rx.try_recv()`: 結果受信時に `in_flight.remove(idx)`
- `in_flight` は main thread 専用。worker thread はアクセスしない

```
Main thread 所有の状態:
  cache     : HashMap<usize, TilePngs>  — レンダリング済み content+sidebar PNG ペア
  in_flight : HashSet<usize>             — 送信済み・未受信の index
  ────────────────────────────────────────────────────────
  tile が cache にも in_flight にもなければ → リクエスト送信可能
  tile が in_flight にあれば             → worker が処理中、待つ
  tile が cache にあれば                 → レンダリング済み、即利用可能
```

追加で、`redraw()` 直前にも `res_rx.try_recv()` を実行し、`event::poll()` のブロック中に worker が完了した結果を回収する。これにより TOCTOU ウィンドウをさらに縮小する。

### イベントループ構造

```
'outer: loop {  ← rebuild loop (初回 + resize)
    tiled_doc = build_tiled_document(...)
    cache = TiledDocumentCache::new()

    thread::scope(|s| {
        s.spawn(worker)  ← prefetch worker (FIFO)
        in_flight = HashSet::new()

        loop {  ← inner event loop
            res_rx.try_recv() → in_flight.remove + cache.insert  // drain
            event::poll() → handle key/resize
            if dirty {
                res_rx.try_recv() → drain (追加: TOCTOU 縮小)
                redraw() + send_prefetch(cache, in_flight)
            }
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
4. worker: レンダリング中なら完了まで待つ（`&tiled_doc` はまだ生存）→ `recv()` が `Err` → 終了
5. scope 完了 → outer loop continue → 新 `tiled_doc` + 新 scope + 新 worker

レイテンシ: worker がレンダリング中の場合、scope join で最大 ~200ms 待つ（1タイル分）。実用上問題なし。

### スレッド安全性

- `Frame.items` は `Arc<LazyHash<Vec<...>>>` — clone は refcount bump のみ
- `typst_render::render(&Page, f32)` — ステートレス、ローカル Pixmap に描画
- `Content::empty()` — `RawContent` は `unsafe impl Send + Sync`
- → `TiledDocument` は `Send + Sync`（コンパイラが検証）

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

### Ghostty 固有の制約（spike_kitty で検証済み、spike_kitty は削除済み）

Phase 4 初期の技術検証用スパイク (spike_kitty) での実機検証で判明した Ghostty の挙動:

1. **`\x1b[2J` 禁止（唯一の地雷）**: 画像データごと消える。配置の削除は `a=d,d=i,i=ID`（小文字 `i` = データ保持）を使う。
2. **`a=T` / `a=t` どちらもOK**: `a=T` でも `i=` 付きならキャッシュされ `a=p` で参照可能。
3. **スクロール手順**: `a=d,d=i` → `a=p` の繰り返し。画像再送信は不要。

詳細は `docs/kitty-graphics-protocol.md` の「Ghostty 固有の注意点」セクションを参照。

### `q=2` による Kitty レスポンス全抑制

Kitty Graphics Protocol の `q` パラメータ:

| 値 | 挙動 |
|---|---|
| `q=1` | OK 応答を抑制、エラー応答は送信 |
| `q=2` | OK・エラー両方の応答を抑制 |

tview は全コマンドで `q=2` を使用する。

**理由**: `q=1` ではエラー応答（例: 画像サイズ上限超過時の ENOENT）が APC シーケンスとして端末に返される。crossterm はこれをキーイベントとして誤解析し、`g`/`G`/`d`/`u` 等のファントムキー入力が発生してスクロールが暴走する（Bug 3）。tview は Kitty レスポンスを処理するコードを持たないため、`q=2` で全応答を抑制しても機能上の影響はない。

### サイドバーのタイル分割

サイドバー（行番号画像）はコンテンツと同じタイル境界で分割される。

**問題**: サイドバーを全ドキュメント高さの1枚画像として送信すると、Ghostty の画像サイズ上限を超えてアップロード失敗する。エラーレスポンスが crossterm にファントムキーイベントとして流入し、スクロール暴走を引き起こす（上記 `q=2` の項を参照）。

**解決策**:

```
content_doc  = compile_document(theme + content)     // コンテンツ Typst コンパイル
visual_lines = extract_visual_lines(content_doc)     // 行番号抽出
sidebar_doc  = build_sidebar_doc(visual_lines, ...)  // サイドバー Typst コンパイル（1回だけ）
tiled_doc    = TiledDocument::new(content_doc, sidebar_doc, ...)  // 両方を同じ境界で分割
```

- サイドバーの Typst ソースは初期化時に1回だけコンパイル
- コンテンツと同じ `split_frame()` で Frame を分割
- worker は `render_tile()` + `render_sidebar_tile()` をセットで実行
- `TilePngs { content, sidebar }` としてキャッシュ
- 配置時はコンテンツとサイドバーそれぞれに Kitty image ID を割り当て、同じ `visible_tiles` 結果で配置

### ファーストビュー最適化

タイルベースレンダリング + prefetch で解決済み:
- ドキュメントは `height: auto` で1ページにコンパイル後、Frame ツリーを垂直タイルに分割
- ビューポート内のタイルのみオンデマンドで PNG レンダリング
- 隣接タイルをバックグラウンドスレッドで先行レンダリング
- ピークメモリはタイルサイズに比例（ドキュメント全体ではない）

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
