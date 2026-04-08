# Reload Performance Improvement Plan

Watch 対象ファイル更新時の再描画レイテンシ改善。
リリースビルドのログ分析に基づく。

## 現状のタイムライン (README.md, release build)

```
file change detected           +0ms
├─ sandbox + image load        +17ms
├─ markdown → typst            +18ms
├─ typst::compile              +72ms   ← 避けがたいコスト
├─ visual_line + diff + tile   +77ms
├─ fork IPC                    +96ms
├─ tile 0 PNG render           +167ms  ★ ボトルネック
├─ Kitty upload + display      +306ms  ★ ボトルネック
└─ prefetch start              +306ms
```

- build_tiled_document: 62ms (typst::compile 54ms が支配的)
- tile 0 PNG render: 70ms
- Kitty 転送: ~139ms (1MB+ base64 → 4KB chunk)
- tile cache: recovered 0/6 (全タイルのハッシュが変わっている)

## 改善案

### 1. Span 除外タイルハッシュ — タイルキャッシュを実効化する

**問題:** `compute_tile_pair_hash()` は typst `Frame` の derive `Hash` を使用。
これには `Span` (ソース位置) が含まれるため、Markdown を1行編集するだけで
それ以降の全 Span がずれ、全タイルのハッシュが変わる。
結果、`merge: recovered 0/6` で毎回全タイル再レンダリングが走る。

**方針:** Frame tree を walk し、視覚要素 (位置・サイズ・描画内容) のみを
ハッシュに含め、Span を除外するカスタムハッシュ関数を実装する。

**期待効果:** 編集箇所を含まないタイルのキャッシュヒットが期待でき、
そのタイルの PNG レンダリング (70ms) + Kitty 転送 (~139ms) をスキップできる。
ドキュメント末尾付近を編集しない限り、先頭タイルは高確率でキャッシュヒットする。

**リスク:** typst の Frame 内部構造に依存するため、typst バージョンアップ時に
追従が必要。ただし derive Hash が動くということは Hash trait 自体は安定。

**対象ファイル:**
- `src/frame/tile.rs` — `compute_tile_pair_hash()`
- テスト追加

### 2. リロード時の即時フィードバック

**問題:** ファイル変更検知からパイプライン完了・タイルレンダリング・Kitty 転送
まで約 306ms、画面は古い内容のまま。操作に対する応答がないため「もさっと」感じる。

**方針:** ファイル変更を検知した直後 (パイプライン開始前) にステータスバー領域に
"reloading..." 等のインジケータを表示する。数 ms 以内に完了するため、
ユーザーには即座に反応したように見える。

**期待効果:** 実際のレイテンシは変わらないが、体感レイテンシを大幅に改善。
「反応した」ことが伝わるだけで印象が変わる。

**対象ファイル:**
- `src/viewer/mod.rs` — reload 検知直後のフロー
- `src/viewer/terminal.rs` or `src/viewer/display_state.rs` — ステータス表示

### 3. Viewer 用 PNG 低圧縮エンコード

**問題:** `render_frame_to_png()` は `pixmap.encode_png()` (tiny-skia) を使用。
デフォルトの zlib level 6 で圧縮され、エンコードに ~70ms かかる。
Kitty 転送先はローカルターミナルなので高圧縮の意味が薄い。

**方針:** `png` crate を直接使い、圧縮レベルを制御可能にする。
Viewer 用は level 1-2 (fast)、`render` サブコマンド (ファイル出力) 用は
level 6 (default) と使い分ける。

**期待効果:**
- PNG エンコード時間: 70ms → 20-30ms 程度 (圧縮レベル依存)
- ファイルサイズ増加: 1MB → 1.5-2MB 程度 (Kitty 転送時間は微増するが、
  エンコード時間短縮の方が大きい)
- 正味 30-40ms の短縮

**注意:** tiny-skia の `encode_png()` は引数なし。`png` crate で
`Pixmap::data()` (raw RGBA) を受け取ってエンコードする形になる。

**対象ファイル:**
- `src/frame/render_png.rs`
- `Cargo.toml` — `png` crate 追加

### 4. Kitty 転送チャンクサイズ増加

**問題:** 現在 `CHUNK_SIZE = 4096` で、1MB の base64 データを ~333 チャンクに
分割して write + flush している。各チャンクに escape sequence のオーバーヘッドあり。

**方針:** CHUNK_SIZE を 16384 (16KB) 等に増やし、syscall 回数を 1/4 にする。

**期待効果:** 数 ms 程度の短縮。単体では小さいが、他の施策との組み合わせで有効。
Kitty 側の制限は特にない (任意サイズのペイロードを受け付ける)。

**対象ファイル:**
- `src/viewer/terminal.rs` — `CHUNK_SIZE` 定数

## 実装順序

1. **即時フィードバック** — 難易度低・体感効果大
2. **Span 除外ハッシュ** — 最大効果だが実装量あり
3. **PNG 低圧縮** — 難易度低・確実な短縮
4. **チャンクサイズ** — 簡単に試せる微調整
