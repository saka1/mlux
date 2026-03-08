# Bug: "15行目が潰れてしまう" 調査記録

## 報告

`RUST_LOG=debug cargo run -- --log tmp/mlux.log tmp/spec.mod.txt` でビューアを開くと、
15行目が潰れて見える。まるで14行目が上にあるかのように。
レンダリング幅に依存する可能性あり。

## 入力ファイル

`tmp/spec.mod.txt` — CommonMark Spec の冒頭部分（102行）。
YAML frontmatter (`---`...`...`) 付き。pulldown-cmark は YAML frontmatter を解釈せず、
`---` を ThematicBreak（水平線）、残りをプレーンテキストとして処理。

## 調査済み項目

### 1. PNG レンダリング内容 ✅ 問題なし

`cargo run -- render` で 400pt, 592pt, 660pt の各幅で出力。
PNG 画像を目視確認 — テキスト行の重なりやつぶれは見られない。

### 2. Visual line の y 座標 ✅ アノマリーなし

`tests/debug_vlines.rs` で 400pt ～ 912pt の9種の幅で visual line を抽出。
- 全幅で連続 visual line の最小 gap = 20.8pt（正常）
- y 座標の重複・近接（< 10pt）は見られない
- テスト幅: 400, 500, 550, 592, 600, 660, 700, 800, 912

### 3. Frame tree ダンプ ✅ 重なりなし

660pt と 400pt でフレームツリーをダンプ。
Text/Group 要素の y 座標に重複なし。
`#line(length: 100%)` と最初のテキストの gap は 14.4pt で正常。

### 4. `has_dominant_child_group` による不要な再帰 🔍 軽微

660pt 幅で、インラインコード `Markdown.pl` を含むパラグラフ行の Group が
`has_dominant_child_group` = true となり、再帰処理される。
しかし、グローバル dedup（5pt tolerance）で同一行にマージされるため、
余分な visual line は最終的に生成されない。
y 座標が 248.2pt → 246.2pt に 2pt ずれるのみ。

### 5. split_frame のタイル分割 ✅ 正常

`Frame::hard` で作成されるため、タイル境界で正しくクリップされる。
コンテンツとサイドバーは同じ `tile_height_pt` で分割。
`tile_actual_height_px` の計算は TiledDocument と DocumentMeta で一致。

### 6. Kitty Graphics Protocol: Single case ✅ 正常

初期表示（y_offset=0）は必ず Single tile。
`vp_h = image_rows * cell_h` なので `r = image_rows`、1:1 スケーリング。

### 7. Kitty Graphics Protocol: Split case ✅ 根本原因特定・修正済み

`place_tiles` の Split case では `top_rows = round(top_src_h / cell_h)`。
`round()` による丸めで上下タイルが独立にスケーリングされ、タイル境界で
微小なスケーリング差が生じる。

**根本原因**: `round()` が切り捨て方向に丸めた場合、
`top_src_h > top_rows * cell_h` となり、Kitty プロトコルが
ソース領域を表示領域に圧縮（scale down）する。
これにより行が「潰れて」見える。

`tests/debug_vlines.rs::debug_split_scaling` による定量分析:
- 最悪ケースで 14.3% の圧縮（cell_h=16 時）
- cell_h=16 の場合、top_src_h=8 → top_rows=1 → display=16px, 2倍引き伸ばし等

**修正** (2箇所):
1. `src/viewer/mod.rs`: `tile_height_pt` を `cell_h` の倍数にアラインメント
   - `tile_height_px = div_ceil(raw, cell_h) * cell_h`
   - タイル高さが cell_h の倍数になるため、タイル境界が常にセル境界と一致
2. `src/viewer/effect.rs`: `ScrollTo` で `y_offset` を `cell_h` の倍数にスナップ
   - `y_offset = (y / cell_h) * cell_h`
   - `visual_line_offset` や `max_scroll` からの非アラインメント値を修正

**結果**: `tile_height_px % cell_h == 0` かつ `y_offset % cell_h == 0` なので、
`top_src_h = remaining_in_top = tile_height_px - src_y_in_tile` も `cell_h` の倍数。
`round(top_src_h / cell_h)` は常に正確な整数除算となり、1:1 スケーリングが保証される。

### 8. サイドバー配置 ✅ 問題なし

サイドバーの `#place()` は visual line の y_pt - 6pt に配置。
連続 visual line の gap は最小 20.8pt で、8pt テキストの重なりは起きない。
サイドバーとコンテンツの列は重なっていない（sidebar: col 0-5, content: col 6-79）。

## 解決済み

根本原因は項目 7 で特定。修正は 2 ファイル・計 6 行の変更。
全テスト（258件）パス、clippy 警告なし。

## 再現用テストファイル

- `tests/debug_vlines.rs` — 各幅での visual line 一覧出力
- `tmp/spec.mod.txt` — 入力ファイル
- `tmp/dump.txt` — 660pt でのフレームツリーダンプ
