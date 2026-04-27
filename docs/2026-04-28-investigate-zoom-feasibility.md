# 調査: ズーム機能の実装方式

**対象**: render パイプライン（`compile/world.rs`, `themes/`）と viewer の文字サイズ動的変更
**実施日**: 2026-04-28
**結論**: **β 方向（テーマ pt 値の scale 化）が推奨**。γ 方向（`#scale(reflow: true)` ラッパ）はレンダリング品質では完全等価だが、tile.rs と非互換。

## 背景

ターミナルサイズが固定で与えられる viewer において、ドキュメントや視力の都合で文字サイズを動的に変更したい。期待ユースケースは「表示された文字のサイズが期待と違っていた」ケースで、ブラウザの Ctrl++ 相当のリフロー型ズーム（行幅は端末に揃え、文字を大きくしたら 1 行あたり文字数は減る）。

実装ターゲットは **viewer のホットキー**（`+`/`-`/`0`）による runtime ズーム。CLI フラグは初期値設定として後付け可能。

## 検討した 3 アプローチ

### α. page width を縮めるだけ
prefix の `#set page(width: ...)` の値を `width / scale` に変えるだけ。テーマも markup も無変更。

- ⭕ 実装 1 行
- ❌ 画像/Mermaid は `#image()` 自然サイズ（PNG: pixel-as-pt at 72dpi、SVG: 属性値）で挿入されるため、page を縮めても画像は元 pt のまま → ズーム時に相対的に巨大化、はみ出し

### β. テーマ全 pt 値 × scale
4 テーマファイル（`catppuccin*.typ`）の全 `Npt` 値に scale を乗算。`Nem` は font size 連動なので自動追従。

- ⭕ Frame 座標が visual = layout で一致 → tile.rs は無変更で動く
- ❌ CLAUDE.md の「4 テーマ lock-step」制約に抵触。scale 引数取り回しを theme 関数化するリファクタが必要
- ❓ 画像挿入も scale 必要（後述）

### γ. `#scale(reflow: true)` で content 全体をラップ
world.rs prefix の content 部分を `#scale(150%, reflow: true)[...]` で包む。

- ⭕ テーマ・markup・画像挿入すべて無変更
- ⭕ 画像/Mermaid は transform に乗って一緒にスケールする
- ❓ Typst の `#scale` 意味論が「ズーム」と一致するか要検証 — これが本実験の主目的

## 実験設計

### Hypothesis
H1: `#scale(reflow: true)` は β（テーマ pt × scale）と pixel 等価な出力を生成する
H2: γ は mlux の既存パイプライン（tile.rs, visual_line.rs）と互換

### Setup
- scale = 1.5 で固定
- 同一 Markdown を γ・β 双方でレンダリング、pixel diff
- テスト fixture: `tests/fixtures/09_all_features.md`（テキスト中心、見出し・コード・リスト・表・数式を網羅）

### 仮実装（実験後 revert 済み）
1. `world.rs::MluxWorld::new` に環境変数 `MLUX_EXPERIMENTAL_SCALE` を追加。値があれば content を `#scale(N%, reflow: true)[...]` で wrap
2. `themes/catppuccin-x15.typ` を新規作成 — `catppuccin.typ` をベースに、scale ラッパー内に位置する pt 値だけを 1.5 倍（margin と page width はラッパー外なので不変）
3. `theme.rs` に `catppuccin-x15` エントリを一時登録

実行:
```bash
# γ
MLUX_EXPERIMENTAL_SCALE=1.5 mlux render fixtures/09_all_features.md \
  --theme catppuccin -o gamma.png
# β
mlux render fixtures/09_all_features.md \
  --theme catppuccin-x15 -o beta.png
```

## 計測結果

### Rendering 等価性（H1 の検証）

`--tile-height 10000` で **単一タイル**として出力し、tile 分割の影響を排除して比較：

| 比較 | 異なる pixel / 全 pixel | 比率 |
|---|---|---|
| γ (default origin) vs β | 51,116 / 9,989,760 | 0.51% |
| γ (origin: top + left) vs β | **21,330 / 9,989,760** | **0.21%** |

差の正体は完全に sub-pixel anti-aliasing（テキストエッジのみで均一に分布）。視覚的には区別不可能。

**H1 は成立**: `#scale(reflow: true)` ≡ テーマ pt scaling、pixel レベルでほぼ完全一致。

サイドバイサイド比較（上部抜粋）:

```
γ (single-tile):                    β (single-tile):
mlux 全機能テストドキュメント        mlux 全機能テストドキュメント
すべての対応済み Markdown 機能を…     すべての対応済み Markdown 機能を…
見出しレベル                         見出しレベル
第3レベル見出し                      第3レベル見出し
…完全に同じレイアウト                 …完全に同じレイアウト
```

### tile 分割互換性（H2 の検証）

mlux のデフォルト動作（`--tile-height 500pt`、約 1000px ピクセル/タイル）で出力した結果：

| 比較 | tile 数 | 異なる pixel | 内訳 |
|---|---|---|---|
| γ (default origin) vs β | 8 / 8 | 199,283 (2.0%) | tile 0 と tile 7 が大量に異なる |
| γ (origin: top+left) vs β | 8 / 8 | 149,439 (1.5%) | tile 6, 7 が空 |

**H2 は不成立**: γ と β は同タイル数を生成するが、γ はタイル境界で content の欠落が起こる。

#### Frame ツリー比較（決定的証拠）

β (theme pt scaling):
```
Group (40.0,  40.0) pt  529.4 ×  26.4   ← H1
Group (40.0,  98.8) pt  580.0 ×  13.2   ← intro paragraph
Group (40.0, 156.1) pt  189.0 ×  23.1   ← H2 "見出しレベル"
Group (40.0, 217.0) pt  218.4 ×  19.8   ← H3
…約 60 個の top-level group が y=40..3744 に並ぶ
```

γ (`#scale(reflow: true, origin: top + left)`):
```
Group (40.0, 40.0)pt   386.7 × 2469.3   ← 単一巨大 Group が全コンテンツを内包
  Group (40.0, 40.0)pt   352.9 ×  17.6  ← H1
  Group (40.0, 79.2)pt   386.7 ×   8.8
  …
```

#### 問題の根本

`#scale(reflow: true)` は **transform 付きの単一 Group** を生成する。この Group は：

- 内部的に **layout 座標**（unscaled = 386.7 × 2469.3pt）でサイズ・位置を保持
- レンダリング時に **visual 座標**（580 × 3704pt = 1.5x）に拡大して描画される

一方 `tile.rs::split_frame` は：
- 親 page frame の `frame.size().y.to_pt()` を **visual 座標**（3784pt）で取得 → タイル境界は visual で計算
- 子 item の境界判定は `pos.y + size.y` を **layout 座標**で取得

この座標系不整合により：

| origin 設定 | tile 0 の Group 報告位置 | tile 6,7 の判定 | 結果 |
|---|---|---|---|
| デフォルト (center) | y=657.3〜3126.6 (layout) | OK | tile 0 が空（657.3 > 500） |
| top + left | y=40〜2509.3 (layout) | NG | tile 6,7 が空（2509.3 < 3000） |

origin をどう設定しても visual 範囲（40〜3744）は layout 範囲（40〜2509）より大きいため、必ず一部のタイルが空になる。

#### 視覚的な結果

γ (default origin) の tile 0:
- 期待: H1 + intro + headings（β tile 0 と同等）
- 実際: 真っ黒（content が tile 1 以降に押し出され、しかも一部欠落して見える）

タイルを連結（`convert tile-*.png -append`）すると、content が縦方向で欠落しているのが確認できる。一方 β は `tile 0..7` を連結すると完全な文書が再現される。

## 関連する波及問題

γ を採用した場合、tile.rs 以外にも影響範囲がある：

- **visual_line.rs**: Frame ツリーを再帰探索して visual line を抽出。Group の transform を考慮する必要がある（現状は親座標系をそのまま伝播）
- **highlight.rs**: text item 位置から highlight 矩形を計算。transform 適用後の visual 座標が必要
- **tile_cache.rs / TileHash**: BLAKE3 ハッシュ計算が Frame tree walk に依存。transform 付き Group の取り扱い要検討

つまり γ は「世界観としては正しい」が、mlux の Frame tree 直接アクセス前提の設計と相性が悪い。Typst の Frame API が transform を「内部的に解決済み」として visual 座標で公開してくれれば自然に動くが、現状は layout 座標が露出している。

## 推奨アプローチ: β

### 採用根拠

1. **tile.rs / visual_line.rs / highlight.rs / TileHash すべて無変更で動く** — 既存テスト 449 件を壊さない
2. テーマ書き換えのコストは一度きり（4 ファイル × 8〜10 個の pt 値）
3. CLAUDE.md の lock-step 制約は **scale 化を全テーマで揃える形で温存可能**

### 実装スケッチ

#### Phase 1: テーマの parametric 化

各テーマファイルの先頭で `let scale = 1.0` を定義し、全 pt 値を `(N * scale)` 形式に：

```typst
#let scale = 1.0
#set page(width: 660pt, height: auto, margin: 40pt, fill: rgb("#1e1e2e"))
#set text(font: "Noto Sans JP", size: 12pt * scale, ...)
#show heading.where(level: 1): it => text(24pt * scale, ...)
…
```

**margin と page width は scale 化しない**（実験で確認した β 方針と整合）。これは「scale はラッパー内のみ適用」という γ の意味論を踏襲したもの。

`em` ベースの値（leading, list indent, raw inline）は font size 連動で自動追従するため変更不要。

#### Phase 2: prefix での scale inject

`world.rs::MluxWorld::new` で width 同様に scale もパラメータ化：

```rust
let prefix = format!(
    "{theme_text}\n{mitex_compat}\n#set page(width: {width}pt)\n#let scale = {scale}\n"
);
```

ただし `#let scale` の値は theme 内の `let scale = 1.0` を上書きできないため、theme 側を `#let scale = scale` にして prefix から override する形にするか、もしくは theme から `let scale` を抜いて prefix だけが定義する形にする。後者の方がシンプル。

#### Phase 3: 画像挿入の scale 化

`compile/markup_util.rs::typst_image()` を修正：

```rust
// before
format!("#align(center)[#image(\"{escaped}\")]\n")
// after
format!("#align(center)[#image(\"{escaped}\", width: image-natural-width(\"{escaped}\") * scale)]\n")
```

ただし Typst で画像の natural width を取得するには `image()` のリターンを measure する必要があり面倒。代替案として **画像も `#scale(scale * 100%, reflow: true)` で個別ラップ**する。これなら全体ラップの γ と違い、画像単体は単純な Group なので tile.rs と整合する：

```typst
#align(center)[#scale(scale * 100%, reflow: true)[#image("path")]]
```

画像単体ラップは tile.rs を破壊しない（Group が画像 1 個だけを内包し、その visual サイズと layout サイズの差は親レベルでは見えない）。

#### Phase 4: 配管

- CLI: `mlux render --scale 1.5` フラグ追加
- viewer: `+` / `-` / `0` ホットキーで scale 変更 → タイルキャッシュ全無効化 → 再コンパイル
- config: `Config::scale: f64` を追加

### 実装の重さ見積

- Phase 1（テーマ parametric 化）: 4 ファイル × 30 行程度の修正、1〜2 時間
- Phase 2（prefix）: 数行
- Phase 3（画像）: 1 関数修正 + テスト
- Phase 4（CLI / viewer）: scroll-acceleration と同程度のリファクタ規模、半日
- Phase 5（テスト）: integration 既存テストの scale=1.0 維持確認、scale=1.5 用の golden image 追加

合計で 1〜2 日程度。

## 残課題・要検討事項

1. **scale の取りうる値の範囲**: 0.5 〜 3.0 程度？刻みは連続か離散（0.1 刻み？プリセット S/M/L/XL？）か
2. **scale × width の組み合わせ**: viewer はターミナル幅から width を決めるが、scale 変更時に width を保つか scale に応じて変えるか
3. **viewer ホットキーの優先度**: 既存キーマップ（mode_normal.rs）との衝突確認
4. **タイルキャッシュのライフサイクル**: scale 変更は実質 reload と等価。既存の reload パスに乗せるのが自然か
5. ~~**画像 `#scale` ラップの安全性**~~: → 後述「画像個別ラップの追加検証」で検証済み（H3 成立、ただし要件あり）

## 画像個別ラップの追加検証（H3）

Phase 3 で採択した「画像単体を `#scale(reflow: true)` でラップする」方式が、PNG・Mermaid SVG・tile.rs と本当に整合するかを検証した（実施日: 2026-04-28、scale = 1.5）。

### Hypothesis
H3: `typst_image()` が出力する markup を `#align(center)[#scale(150%, reflow: true)[#image("path")]]` に書き換えるだけで、PNG/Mermaid SVG 双方が正しくスケールされ、tile.rs に content loss を起こさない

### Setup

`src/compile/markup_util.rs::typst_image()` に env var `MLUX_EXPERIMENTAL_IMAGE_SCALE` を追加し、有効時のみラップを挿入（10 行）。テーマや prefix は触らないため、テキスト要素は scale=1.0 のまま、純粋に画像挿入のみの影響を測定できる。

fixture:
- `tests/fixtures/11_image.md`（1×1 〜 1200×600 の PNG 6 枚）
- `tests/fixtures/13_mermaid.md`（mermaid SVG 3 枚）

### 計測結果

#### スケール挙動（PNG）

| 画像 | 自然サイズ (pt) | baseline 出力 (pt) | scale=1.5 出力 (visual pt) | 期待 1.5x | 実測 |
|---|---|---|---|---|---|
| 1×1 | 1×1 | 1×1 | 1.5×1.5 | ✓ | ⭕ |
| 32×32 | 32×32 | 32×32 | 48×48 | ✓ | ⭕ |
| 128×128 | 128×128 | 128×128 | 192×192 | ✓ | ⭕ |
| 300×200 | 300×200 | 300×200 | 450×300 | ✓ | ⭕ |
| 800×400 | 800×400 | **580×290**（page 幅で clamp） | **580×290** | △ | ⚠️ scale なし |
| 1200×600 | 1200×600 | **580×290**（同上） | **580×290** | △ | ⚠️ scale なし |

ページ全体高さ: baseline 2842px → scaled 3203px。差分は小画像 4 枚の visual 拡大ぶん（大画像 2 枚は元から page 幅 clamp で頭打ち）。

**重要な制約**: `#scale(reflow: true)` の内部レイアウトは「visual サイズが親の利用可能幅 (580pt) を超えない」よう逆算される。具体的には `inner_layout_width = 580 / scale = 386.7pt` が画像に与えられ、画像はその幅に収まる aspect ratio で layout される（→ visual 386.7×193.3 × 1.5 = 580×290）。結果として **既に page 幅まで拡大されていた画像は scale が効かない**。これは zoom の「文字も画像も等比拡大される」という素朴な期待と乖離する。

> 回避策の選択肢:
> 1. 受容: 「page 幅を超える画像は元から clamp」という既存挙動の自然な拡張と捉える
> 2. page width も scale に応じて拡げる（例: viewer のターミナル幅から逆算する width 計算側で吸収）— β 全体の整合性を要再検討
> 3. `#image(width: natural-pt * scale)` で個別に幅指定する — Typst で natural width を取得する手段が無く、`measure()` をかます必要があり実装重め

#### スケール挙動（Mermaid SVG）

mermaid 3 種すべてが page 幅未満（最大 333.8pt）のため、**全 SVG が visual 1.5x で正しく拡大**された。

ページ高さ: baseline 929.6pt → scaled 1174.3pt（+244.7pt）。SVG 合計高さ 489.3pt × 0.5 = 244.65pt と完全一致。

#### tile.rs 互換性

default tile-height（500pt）でタイル分割した結果を `convert -append` で連結し、単一タイル出力と pixel diff:

| | tile 数 | concat vs single の異 pixel | 比率 |
|---|---|---|---|
| baseline (PNG) | 3 | 3,961 / 3,751,440 | 0.001% |
| scaled (PNG) | 4 | **5,716 / 4,227,960** | **0.001%** |
| baseline (mermaid) | 2 | 3,535 / 2,453,880 | 0.001% |
| scaled (mermaid) | 3 | **543 / 3,100,680** | **0.0002%** |

差分は完全に sub-pixel anti-aliasing で、γ で発生したような content の欠落・タイル全黒・コンテンツ押し出しは **一切観測されなかった**。

#### Frame ツリー観察

scale=1.5 適用後の wide_1200 PNG の Frame:

```
Group (136.7, 1319.6)pt  386.7x193.3pt    ← layout 座標で 386.7×193.3 (= visual / 1.5)
  Image (136.7, 1319.6)pt  386.7x193.3pt
```

γ と同じ「**Group の報告サイズが layout 座標**（386.7×193.3）で、visual 描画は 1.5x（580×290）」という座標系不整合が生じている。にもかかわらず tile.rs が壊れない理由：

1. **Group が画像 1 個のみを内包** → 不整合の影響範囲が `(visual_h - layout_h) / 2 ≈ 48pt` 程度（scale=1.5 のとき）に限定
2. **親 page 側の flow は visual サイズで space 確保**（次の Tag 位置が正しく後ろにずれる）→ tile_count 計算（`page.size().y`）と tile 境界は visual で正しい
3. **layout 範囲を overlap する全タイルに Group が clone される**: 多くの場合これが visual 範囲もカバーする
4. **tiny-skia がタイル外を clip** → scale transform 込みの描画が境界をまたぐ場合も clip により正しい部分だけが見える

ただし **理論上の edge case**: visual 範囲が layout 範囲より大きいため、layout 範囲外（visual 範囲内）の薄い「halo」だけがちょうど 1 タイルに収まる場合、そのタイルから Group が pull されず content が抜ける。具体的には:
- 32×32 PNG (layout 32, visual 48, halo = 8pt 上下) を scale=1.5 でラップしたとき、画像上端が layout(194.5) でも visual(186.5) でも、その間 8pt 幅にだけかかるタイル（≤ 8pt の極端な tile-height）は欠ける可能性がある

実用的なズーム範囲（scale ≤ 2.0）と tile-height（≥ 100pt）では発生しない。**安全マージン: `tile_height_pt ≥ 4 × max_image_height_pt × (scale - 1) / 2`**。

#### visual_line.rs / TileHash への影響

- `visual_line.rs::collect_visual_lines_structural` は `FrameItem::Image` を無視し、画像 1 個だけを内包する Group は `should_recurse = false` ＋ spans 空 → visual_line を emit しない。**baseline と同じ挙動** ✓
- `TileHash` の BLAKE3 walk: Group が 1 階層増えるためハッシュは変わるが決定的。scale 値ごとに異なるキャッシュエントリになる（scale 切り替え時の cache invalidation は意図通り）

### 結論

- **H3 成立**: 画像個別ラップは PNG・Mermaid SVG いずれにも適用可能。tile.rs は無変更で正しく動作する
- **要件**: 「page 幅 clamp された画像は visual scale が効かない」という挙動を許容すること。zoom の意味論を「テキストは確実に scale、画像は page 幅まではテキストと同期して scale、page 幅以上は据え置き」と整理すれば自然
- **追加対応不要**: visual_line.rs / TileHash / highlight.rs はいずれも修正不要

### 検証で実施した変更（revert 済み）
- `src/compile/markup_util.rs::typst_image()`: `MLUX_EXPERIMENTAL_IMAGE_SCALE` env var 対応（11 行）

### 生成物（`/tmp/mlux-image-scale-exp/` に保存、未コミット）
- `baseline_image*.png`, `scaled_image*.png`: PNG fixture の baseline / scaled 出力
- `baseline_mermaid*.png`, `scaled_mermaid*.png`: Mermaid fixture の同上
- `*_dump.txt`: Frame ツリーダンプ
- `*_concat.png`: タイル連結出力（content loss 確認用）

## 実験ログ

### 実施した変更（すべて revert 済み）
- `src/compile/world.rs`: `MLUX_EXPERIMENTAL_SCALE` env var 対応（10 行）
- `src/theme.rs`: `catppuccin-x15` エントリ追加（30 行）
- `themes/catppuccin-x15.typ`: 新規 1.5x スケール済みテーマ（57 行）

### 生成物（`/tmp/mlux-zoom-exp/` に保存、未コミット）
- `gamma_*.png`, `beta_*.png`: タイル分割出力
- `gamma_single*.png`, `beta_single*.png`: 単一タイル出力
- `*_dump.txt`: Frame ツリーダンプ
- `diff_*.png`: pixel diff 可視化

### 確認した品質ゲート
- `cargo build --release`: 成功
- `cargo clippy --all-targets`: 警告なし
- `git status`: clean
