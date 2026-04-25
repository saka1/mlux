# 実験: コンテンツ列幅キャップ（不採用）

**対象**: render パイプライン (`mlux render`) と viewer の本文 page 設定
**実施日**: 2026-04-25
**結論**: **不採用**。実装の効果はあるが、UX 上のメリットが介入コストに見合わない。

## 背景

[`docs/2026-04-25-investigate-typst-typography.md`](2026-04-25-investigate-typst-typography.md) §1 で「現行 580pt / 12pt ≈ 96 cpl は Bringhurst (45-75) / Butterick (45-90) の上限超過」「48 全角字は JLReq 横組み 40 字以下推奨を超過」と指摘されていた。これに対する対応案として:

- (a) ページ幅を狭める (480-540pt)
- (b) 本文 font-size を縮小 (11pt)
- (c) マルチカラム化

の 3 案が示されていたが、(a) は「mlux がページ幅を typography 判断で決める」ことになり、ブラウザ的な「ビューポートは外部入力」の原則と衝突する懸念が議論された。

## 設計議論の経緯

### 採用した原則: 「mlux はページ幅を決めない」

ブラウザの user agent はビューポートサイズを決めず、CSS 著者がコンテンツ側のレイアウトを決める。これに倣い:

- **viewer**: ターミナル幅 → page 幅（既に実装済み, `viewer/layout.rs:34`）
- **render**: CLI `--width` → page 幅（明示性を担保するため、デフォルト値の意味付けは「typography 判断」ではなく「印刷物理寸法 fallback」に再定義）

### typography 介入の余地: 「page 幅は触らないが column 幅は触る」

ブラウザでも `<body>` を生表示せず、CSS は `max-width: 65ch on <article>` のような column キャップを通常設定する。これに対応するのが本実験の対象 = **(α) コンテンツ列キャップ**。

- ページ幅 = ターミナル / CLI 由来（mlux は決めない）
- 本文 column 幅 = `min(page_width − 2×min_side, max_content)`（theme 側の判断）
- 余剰幅は side margin が吸収 → 中央寄せで「視覚的余白」になる

却下された他の手段:
- **(β) Fluid font-size**: ページ幅に応じて 12-16pt 可変。Utopia.fyi 流。fixed-PPI 出力との相性が悪く未採用
- **(γ) Multi-column**: 「広いとき段組」。挙動が非直感的（スクロール / セレクション / 検索の整合性が崩れる）として早期却下
- **(δ) 何もしない**: cpl はユーザの責任。最終的にこれが採用された（後述）

## 実装プロトタイプ

`src/compile/world.rs:153` で page 幅 override に margin 計算を追加:

```rust
let max_content_pt: f64 = 480.0;
let min_side_pt: f64 = 40.0;
let side_pt = ((width - max_content_pt) / 2.0).max(min_side_pt);
let prefix = format!(
    "{theme_text}\n{mitex_compat}\n#set page(width: {width}pt, margin: (x: {side_pt}pt, y: 40pt))\n"
);
```

- **max_content = 480pt**: CJK 40 全角字（JLReq 横組み上限）に整合
- **min_side = 40pt**: 既存 theme の `margin: 40pt` を踏襲し、狭い幅での visual baseline を維持
- **activation 閾値**: width > 480 + 2×40 = **560pt**。これ以下では baseline と pixel 一致

## 計測結果

`tests/fixtures/08_long_prose.md`（CJK 散文 90 行）を 6 幅で render:

| 幅 | side | content | activation | 視覚的印象 |
|---|---|---|---|---|
| 300pt | 40pt | 220pt | off | baseline と完全一致 |
| 360pt | 40pt | 280pt | off | baseline と完全一致 |
| 480pt | 40pt | 400pt | off | baseline と完全一致 |
| 560pt | 40pt | 480pt | 閾値 | 変化点なし |
| 720pt | 120pt | 480pt | on | 中央寄せが視認できる、適度 |
| 900pt | 210pt | 480pt | on | 40 字/行、JLReq 横組み上限ぴったり |

`tests/fixtures/16_latin_full.md` (Latin) でも検証。900pt で content 480pt → ~85-90 cpl で Butterick 上限ギリギリ、Bringhurst 推奨 (66字) より依然広い。**Latin theme は max_content をもっと狭く（~390pt）した方が良い**ことが判明。理想形は `max_content` を theme 毎の定数として持つ実装。

## 不採用の理由

実装は機能した。狭い幅でも違和感が出ず（cap 非活動領域 = baseline と pixel 一致）、広い幅では明らかに cpl が改善した。にもかかわらず採用しなかった理由:

1. **可読性の体感的向上が小さい**: 「広いビューポートでも文字が密に並ぶ」体験はブラウザで日常化しており、cpl 過多に対する違和感が薄い。Bringhurst の 45-75 字規範は紙組み由来であり、スクロール型 viewer では同程度に load-bearing ではない可能性が高い。
2. **ターミナル側で同等の効果が得られる**: ユーザが column cap 相当を欲しい場合、ターミナルウィンドウを狭めるだけで mlux は自動追従する（viewer pipeline は既にターミナル幅 driven）。ツール側で能動的に介入する必要が薄い。
3. **「mlux は介入しない」原則の純度を優先**: viewer ですでに「ターミナル = page 幅」が成立しており、theme 側で column を狭めると「ユーザが広くしたのに mlux が狭めた」感が発生する。実害は無くても thinking model が一段複雑になる。

つまり議論の到達点は **(δ) 何もしない** だった。元の §1 で「高優先」とされた cpl 過大問題は、設計判断として「ユーザのターミナル設定の問題」へ移管された。

## 残された選択肢（実装するなら）

将来「やはり cpl 制御が要る」となった場合の足場として記録:

- **theme 側に max_content 定数**: `catppuccin.typ` に `#let _mlux_max_content = 480pt`、`catppuccin-latin.typ` に `#let _mlux_max_content = 390pt`。`world.rs` から参照 / 計算。
- **CLI / frontmatter で上書き**: `--max-content=540pt` フラグ、または markdown frontmatter で `max-content: 540pt`。
- **opt-in 化**: デフォルト off、`--narrow` のような明示フラグでのみ有効。原則純度を保ちつつ機能だけ提供する妥協案。

## 元 doc への反映

[`docs/2026-04-25-investigate-typst-typography.md`](2026-04-25-investigate-typst-typography.md) §1 の「measure (欧文) / 行長 (和文) を高優先」は、本実験の結果に基づき **「設計判断としてユーザ責任に移管」** という stance に修正可能。「ベストプラクティス」欄の数値は参考情報として残す。

## 参考

- 原議論ログ: 本セッション (2026-04-25)
- 元 investigate: [`docs/2026-04-25-investigate-typst-typography.md`](2026-04-25-investigate-typst-typography.md)
- ブラウザ流派の参考: [Practical Typography – measure & line-length](https://practicaltypography.com/line-length.html)
