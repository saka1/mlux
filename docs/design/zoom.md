# mlux Zoom Design

## Overview

mlux のターミナルビューアにおけるタイポグラフィズーム（`+` / `-` / `=` ホットキー、
`--scale=N` CLI フラグ）の設計判断を、なぜそうなっているかの根拠と共に記録する。
コードから直接読み取れない設計判断（試したが捨てた選択肢、参照した規範、
トレードオフの構造）を残すのが目的。

調査・実験ログは `../2026-04-28-investigate-zoom-feasibility.md` を参照。
本ドキュメントは「採択した方式」の側に寄る。

## Goals

ターミナルサイズが固定で与えられる viewer において、ドキュメントや視力の都合で
文字サイズを動的に変更したい。期待ユースケースは「表示された文字のサイズが期待と
違っていた」ケースで、ブラウザの Ctrl++ 相当のリフロー型ズーム（行幅は端末に揃え、
文字を大きくしたら 1 行あたり文字数は減る）。

具体要件:

1. **Reflow ズーム**: 拡大したら 1 行の文字数が減る（横スクロールではなく折り返し）
2. **画像も同期して拡大**: テキストだけ大きくして画像が相対的に小さくなる挙動は不自然
3. **離散プリセット**: 連続スライダではなくホットキー 1 回で次のステップへ
4. **100% を anchor として保持**: `=` キーでデフォルトに戻れる
5. **既存の tile.rs / visual_line.rs / TileHash を壊さない**: zoom のために viewer
   コアの座標系を作り直すコストは支払わない

## Architecture: theme parametric scaling (β)

実装方式として 3 つを検討し、**β（テーマ pt 値の scale 化）** を採択した。

### Considered alternatives

| 方式 | 概要 | 採否 |
|---|---|---|
| α | `#set page(width: width / scale)` だけ変える | ❌ 画像が page 幅を超えてはみ出す |
| β | テーマの全 `Npt` 値に scale を乗算、画像は個別に `#scale` ラップ | ⭕ 採択 |
| γ | content 全体を `#scale(N%, reflow: true)` で包む | ❌ tile.rs と非互換 |

#### Why γ fails: layout vs visual coordinate mismatch

`#scale(reflow: true)` は **transform 付きの単一 Group** を生成する。この Group は:

- **layout 座標**でサイズ・位置を保持（unscaled）
- レンダリング時に **visual 座標**（× scale）に拡大して描画される

一方 `tile.rs::split_frame` は親 page frame のサイズを visual で取得して
タイル境界を決めるが、子 item の境界判定は `pos.y + size.y` を **layout** で取得する。
この座標系不整合により、γ ではタイル境界で content が欠落する（実験では `tile 0`
が真っ黒、`tile 7` が空白などの結果が再現的に観測された）。

詳細な Frame ツリーダンプと pixel diff は調査ログを参照。

#### Why β works

テーマの pt 値を直接スケールするので Frame 座標は visual = layout で完全一致。
tile.rs / visual_line.rs / highlight.rs / TileHash すべて無変更で動く。
コストはテーマ書き換え（4 ファイル × ~10 個の pt 値）一度きり。

### Implementation pattern

#### Theme: `#let scale = N` injection

`world.rs::MluxWorld::new` が prefix で `#let scale = N` を inject する。
テーマ側は全 `Npt` を `(N * scale)` の形で書く:

```typst
#set text(font: "Noto Sans JP", size: 12pt * scale, ...)
#show heading.where(level: 1): it => text(24pt * scale, ...)
```

`em` ベースの値（leading, list indent, raw inline）は font size 連動で自動追従。

**margin と page width は scale しない** — 端末幅は固定なので。これは「行幅は端末に
揃え、文字だけスケール」という reflow ズームの意味論と一致する。

#### Image: per-image `#scale(reflow: true)` wrapper

画像の natural size は pt 単位で固定なので、テーマ scale だけでは追従しない。
`compile/markup_util.rs::typst_image()` で画像を個別に `#scale` でラップ:

```typst
#align(center)[#std.scale(150%, reflow: true)[#image("path")]]
```

γ で問題になった「単一巨大 Group が tile.rs を壊す」現象は、**画像 1 個だけを内包する
Group では発生しない**。理由:

1. 不整合の影響範囲が `(visual_h - layout_h) / 2` 程度に限定される
2. 親 page 側の flow は visual サイズで space を確保 → tile 数と境界は visual で正しい
3. layout 範囲を overlap する全タイルに Group が clone される
4. tiny-skia がタイル外を clip → 境界をまたぐ描画も clip により正しい部分だけが見える

実用域（scale ≤ 2.0、tile-height ≥ 100pt）では content loss は観測されない。
理論上の edge case（極端に小さい tile-height）に対する安全マージンは
`tile_height_pt ≥ 4 × max_image_height_pt × (scale - 1) / 2`。

#### `#std.scale` namespace dance

テーマ内で `#let scale = N` と束縛しているため、Typst stdlib の `#scale(...)` 関数が
shadow される。画像ラップ側は `#std.scale(...)` と明示参照することで衝突を回避。
詳細は known-issues.md M1 を参照（cosmetic な footgun だが現状は機能上問題なし）。

## Preset Curve Design

### Constraints

- **Range**: 50%–200%（端末ビューアの用途として 50% 未満も 200% 超も需要なし）
- **Anchor**: 100% は必ずプリセットに含む（`=` reset の到達点）
- **離散ステップ数**: 5〜10 程度（`+` を多重押下する手数を許容できる範囲）

### Reference: Chrome's curve

参考としてブラウザの設計を調査した。Chrome (Chromium) は Ctrl++ で
**25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500%**
の 17 段を循環する。特徴:

- 全体としては 1.2 倍前後の幾何級数だが、**100% 周辺で意図的に細かい**
- `100 → 110` (×1.10), `90 → 100` (×1.11), `100 → 125` で見ると ×1.10〜×1.14
- 端の `25 → 33` (×1.32), `300 → 400` (×1.33) は粗い
- 完全な等比ではない非対称設計

これは Weber-Fechner 則（知覚は対数スケール = 等比ステップが知覚的に均一）を
意図的に違反している。「ユーザは 100% 付近で微調整したい」という
**操作文脈** のほうが優先された設計判断と読める。

### Trial-and-error history

#### v2.3.1 initial: `[0.5, 0.85, 1.0, 1.25, 1.5, 2.0]`

zoom 機能を最初に実装したときの仮置きプリセット。Chrome のような数値感覚で
適当に並べたもの。後にレビューで以下の歪みが指摘された:

| ステップ | 比率 |
|---|---|
| 0.50 → 0.85 | **×1.70** ← 大きすぎ |
| 0.85 → 1.00 | ×1.18 |
| 1.00 → 1.25 | ×1.25 |
| 1.25 → 1.50 | ×1.20 |
| 1.50 → 2.00 | ×1.33 |

`0.50 → 0.85` のジャンプが極端で、Chrome なら間に 2〜3 ステップ置く範囲。
等比でも等差でもない中途半端な構成。

#### Trial 1: `[0.50, 0.63, 0.79, 1.00, 1.26, 1.59, 2.00]` — 等比 7 段

各ステップ ×2^(1/3) ≈ ×1.26。100% を中央に左右 3 段ずつ完全対称、
Weber-Fechner 則的に均一。実装上もシンプルで、提案時点では推奨案だった。

実際に試した結果、**知覚的にジャンプが大きく感じた**。`+` を 1 回押すごとに
読みやすさが「明らかに変わる」量で動き、微調整が効かない。

#### Trial 2 (採択): `[0.50, 0.67, 0.85, 1.00, 1.10, 1.25, 1.50, 2.00]` — Chrome 風 8 段

100% 周辺を密にし、両端を粗にした非対称設計:

| ステップ | 比率 |
|---|---|
| 0.50 → 0.67 | ×1.34 |
| 0.67 → 0.85 | ×1.27 |
| 0.85 → 1.00 | ×1.18 |
| **1.00 → 1.10** | **×1.10** ← 最も細かい |
| 1.10 → 1.25 | ×1.14 |
| 1.25 → 1.50 | ×1.20 |
| 1.50 → 2.00 | ×1.33 |

100% 直近が ×1.10 と最も細かく、端に向かうほど粗い。Chrome の `50–200%` 範囲を
そのまま 8 ステップに pruning した形。

### Insight

Trial 1 → Trial 2 の差し替えで得られた知見:

> **操作文脈では Weber-Fechner よりも「100% 周辺の細粒度」が体験を支配する**

ズームを「読みやすさを 1〜2 回調整して固定する」道具として使うと、
等比ステップでは「ちょうどいい」を通り越してしまうことが多い。
ブラウザ業界が長年かけて辿り着いた非対称設計には合理性があった。

将来のプリセット見直し提案では、等比性だけを根拠にステップ数を減らす
提案はしないこと。

## Known constraints

### Page-width-clamped images don't scale further

Page 幅 (580pt) まで拡大されている画像は `#scale(150%, reflow: true)` でラップしても
visual 出力は変わらない。`#scale(reflow: true)` の内部レイアウトは
「visual サイズが親の利用可能幅を超えない」よう逆算されるため、
`inner_layout_width = 580 / scale = 386.7pt` が画像に与えられ、
visual で 580pt に戻る。

zoom の意味論を**「テキストは確実に scale、画像は page 幅まではテキストと同期して
scale、page 幅以上は据え置き」**と整理すれば自然な挙動。

### Tile cache invalidation on scale change

scale を変えると `TileHash` が変わるため、scale 切り替えは実質的に再コンパイル。
これは意図通り（古い scale の cache を再利用すべきでない）。

## References

- 実装方式の詳細実験ログ: `../2026-04-28-investigate-zoom-feasibility.md`
- テーマのタイポグラフィ設計: `./typography.md`
- 既知の cosmetic 問題: `../known-issues.md`（M1: `scale` 変数 shadow）
