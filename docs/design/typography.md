# mlux Typography Design

## Overview

mlux のテーマ (`themes/catppuccin*.typ`) に埋め込まれているタイポグラフィのマジックナンバー
群を、なぜその値を選んだのかの根拠と共に記録する。コードから直接読み取れない設計
判断（試したが捨てた選択肢、参照した規範、トレードオフの構造）を残すのが目的。

調査・外部仕様の網羅的評価は `../2026-04-25-investigate-typst-typography.md` を参照。
本ドキュメントは「採択した値」の側に寄る。

## Theme matrix

mlux のテーマは 2 軸の 2×2 マトリクスで構成される:

|              | CJK 文書 (lang: ja, Noto Sans JP) | Latin 文書 (lang: en, Fira Sans) |
|--------------|-----------------------------------|----------------------------------|
| **mocha** (dark)  | `catppuccin.typ`                 | `catppuccin-latin.typ`            |
| **latte** (light) | `catppuccin-latte.typ`           | `catppuccin-latte-latin.typ`      |

**Lock-step rule**: 4 ファイルは色とフォント以外で構造的に同一でなければならない。
タイポグラフィ knob を追加・変更する際は 4 ファイル同時に修正する。同軸テーマ（例:
mocha CJK と mocha Latin）の挙動の差は bug とみなす。

## Body text

### Font

- **CJK (lang: ja)**: `Noto Sans JP` 単独
- **Latin (lang: en)**: `Fira Sans` 単独

Typst の font fallback は character-by-character なので、Latin テーマで偶然 CJK 文字が
混在しても fallback が走る（が、設計上は Latin 文書は CJK を含まない前提；prescan で
振り分け済み — `../2026-03-21-latin-mode-design.md`）。

### Size — 12pt

ビューア用途の Markdown レンダリングに広く使われる値。Butterick / Bringhurst の言う
"text size" 範囲（10-13pt）の中央付近で、KGP @ 144 PPI の表示密度とも整合する。

### Page geometry

- ページ幅 **660pt**
- 余白 **40pt** (上下左右共通)
- コンテンツ幅 **580pt**

580pt / 12pt ≈ **48 全角字 / 96 latin cpl**。

- **Bringhurst** (Latin): 45-75 文字、66 字理想 → 96 cpl は上限超
- **Butterick** (Latin): 45-90 字 → やはり超
- **JLReq** (CJK 横組み): 40 字以下推奨 → 48 字は超過

つまり**規範的には行長が長い**。これは意図的なトレードオフで、Bringhurst の規範は
紙の本（読書距離 30-40cm、両ページ視野）を前提とし、画面の単段スクロール（読書距離
40-60cm、片側視野）とは前提が異なる。狭めると tile サイズが小さくなりスクロール体験
が断片化するため、可読性とスクロール UX のバランスを取って現状値を維持している。

行長 narrow の実験は将来候補として `investigate` 側に残す。

### Leading — 0.75em (Latin) / 0.9em (CJK)

**スクリプト別に分岐** (commit `6ec8c87`)。Typst の `par.leading` は line-box 間ギャップ
で、line-box ≈ 1em のため leading ≈ 総 line-height − 1em が近似的に成立。

**Latin 0.75em (~169% 総 line-height)**:
- Butterick 推奨上限 145% を僅かに上回るが、これは Butterick が紙印刷前提のため
- KGP 144 PPI の画面で 145% は詰まって読み取りにくい
- Fira Sans の x-height が高めで、視覚的に "詰まりやすい" のもこちらに振る理由

**CJK 0.9em (~190% 総 line-height)**:
- JLReq §3.2.3: 行長が 35 字を超える場合「1em かそれに近い」を推奨
- 1.0em は試したが slack に感じた経験から「近い」を 0.9em と解釈
- 行長 48 字（kihon-hanmen）は JLReq 上限超過なので、行間を広めに取る方が密度が緩む

**比較ベンチマーク** (詳細は investigate 側 Appendix A):
- GitHub 本文: 150% (= leading 0.5em) — Latin 前提
- Typst デフォルト: ~165%
- mlux 旧設定: 200% (= 1.0em) — JLReq 上限相当だが slack
- mlux 現行 Latin: 169% / CJK: 190%

### Justification

```typst
#set par(justify: true, first-line-indent: 0pt)
```

`linebreaks: "optimized"` は明示しない（`justify: true` のとき `auto` の解決先と同一で、
明示の有無は byte-identical: investigate §3 で SHA256 一致検証済）。

`first-line-indent: 0pt` は和文の伝統的「字下げ」を行わない選択。Markdown ビューア
として段落間に空行を取るのが標準で、字下げと併用すると過剰になる。

## Heading sizes — 24/21/18/16/14/12pt

### History

| 試行 | 値 | 結果 |
|------|----|----|
| 初期 (hand-tuned) | 24/20/16/14/13/12 | 比が不揃い（1.20/1.25/1.143/1.077/1.083）だが感覚的に許容 |
| 純粋 modular scale 1.2 | 12/14.4/17.3/20.7/24.9/29.9 | h1 が 30pt 近く、画面では過大 — 不採用 |
| 純粋 modular scale 1.25 | 12/15/18.75/23.44/29.3/36.6 | h1 = 36.6pt は論外 — 不採用 |
| **コンポジターズスケール (採用)** | **24/21/18/16/14/12** | Bringhurst スケール上の連続値、Weber 則整合 |

### なぜ純粋幾何 (modular scale) は機能しないか

`f_i = body × r^i` は h6→h1 に向けて指数的に肥大する。`r=1.2` でも h1 = 12 × 1.2⁵ = 29.86pt。
これは紙書籍の見開きでは許容されるが、単段スクロールビューアの狭い視野では **document
全体の visual rhythm を h1 が支配してしまう**。スクロール時に h1 だけが視界に飛び込んで、
本文と見出しの readability balance が崩れる。

### コンポジターズスケールとは

伝統的な金属活字鋳造で「実際に在庫されていた」 point size の系列:

```
… 11, 12, 14, 16, 18, 21, 24, 30, 36, 48, 60, 72 …
```

Bringhurst『The Elements of Typographic Style』§3.1 に正式採録。本文 12pt を h6 に
固定して上 5 段を取ると、過不足なく 6 段にはまる。

**ステップ幅** (h6→h1): 2, 2, 2, 3, 3 — 上端で広く、下端で狭まる **Weber 則的等差**。
modular scale (純粋幾何) と異なり、中段で「次のサイズ」感が自然に出る（小サイズの差は
小さく、大サイズの差は大きく感じる、という人間の知覚閾値特性に整合）。

**比** (本文比): 2.00, 1.75, 1.50, 1.33, 1.17, 1.00 — 全て 1/12 の倍数で清潔。reasoning
しやすく、rationale 文書化が容易。

### h6 = 本文サイズ

h6 は本文 12pt と同サイズ。weight (bold) + 色のみで差別化する。これは `13pt` 等の
中間値を回避するため（コンポジターズスケールに 13 は無い）と、Markdown 文書で h6 が
本文に近い役割を担うことが多いという観察による。

## Heading spacing — 1.4em above / 0.9em below (流派 B / proportional)

### 構造

```typst
#show heading.where(level: N): it => text(Npt,
  block(below: 0.9em, above: 1.4em, text(weight: "bold", fill: <color>, it.body)))
```

外側 `text(Npt, ...)` で見出しサイズが先に確立されるため、`block` 内の `1.4em / 0.9em`
は **見出しフォントサイズ基準** で解決される。結果として h1 (24pt) は 33.6pt 上余白 / 21.6pt
下余白、h6 (12pt) は 16.8pt / 10.8pt と、サイズに比例した自動 taper が得られる。

### なぜ流派 B (proportional) を選んだか

見出し余白の決め方には少なくとも 3 流派ある:

- **A. Baseline grid** (Bringhurst / Müller-Brockmann): 余白を本文 leading の整数倍に snap。
  多段組や脚注を持つ印刷物で行のグリッド整合に効く
- **B. Proportional** (Tim Brown / Tailwind / Material Design): 余白を heading フォント
  サイズに比例させる。文字サイズと余白が同一比例系で整い "optical hierarchy" が出る
- **C. Constant-effective** (HTML5 UA stylesheet): em を level 毎に逆スケールして実効値
  を一定に保つ

**mlux の選択は B**。理由:
1. 単段スクロール式の用途で、流派 A の baseline grid 整合の利点（多段組・脚注）が
   効かない
2. modular scale (heading サイズ) との harmony を優先
3. `1em` 一個で全 6 レベルが自動 taper する → 設定の冗長性が減る

### 非対称 (1.4em : 0.9em ≈ 1.55 : 1)

Bringhurst が見出し余白の非対称（above > below）を許容する根拠は **Gestalt proximity**:
見出しは後続コンテンツとグループ化すべきという原理。下が狭く上が広いと、見出しが
直下の本文と知覚的にまとまる（webtypography.net §2.2.2）。

具体的には `1.4em : 0.9em` で **above が below の約 1.55 倍**。比率としては Bringhurst
が例示する `1.5 + 0.5 lines` (= 3:1) より穏やかで、画面の visual density に合わせて緩めた値。

### 4 テーマ統一の経緯

2026-04-25 までは latte 系が `block(..., text(Npt, ...))` 構造（em が body 解決）、mocha
系が `text(Npt, block(...))` 構造（em が heading 解決）と分かれていた。前者は em が
本文 12pt 基準で解決されるため余白が一定、後者は heading フォントサイズ基準で taper
する。同じ 1.4em / 0.9em でも実効余白が違うという theme 間の挙動差 bug があった。
commit `32b6184` で全テーマを後者構造に統一して解消。

## Heading colors

### 役割マッピング (Catppuccin パレット)

| level | mocha 色 | hex | latte 色 | hex |
|-------|---------|-----|---------|-----|
| h1 | Mauve | `#cba6f7` | Mauve | `#8839ef` |
| h2 | Pink | `#f5c2e7` | Pink | `#ea76cb` |
| h3 | **Lavender** | **`#b4befe`** | **Lavender** | **`#7287fd`** |
| h4 | Maroon | `#eba0ac` | Red | `#e64553` |
| h5 | Yellow | `#f9e2af` | Yellow | `#df8e1d` |
| h6 | Subtext1 | `#a6adc8` | Subtext1 | `#5c5f77` |

### h3 Rosewater → Lavender の経緯 (2026-04-25)

旧設定では h3 = Rosewater (`#f5e0dc` mocha / `#dc8a78` latte)。Mocha の Rosewater は
パレット中 **最も明るい色** (相対輝度 ~0.80、ほぼ純白) で、h3 のサイズが 16pt → 18pt に
上がった結果「dark bg 上で h1 より目立つ」現象が発生。

**輝度ヒエラルキー** (旧設定 mocha):

| level | 色 | 輝度 |
|-------|----|------|
| h1 | Mauve | ~0.50 |
| h2 | Pink | ~0.70 |
| **h3** | **Rosewater** | **~0.80** ← 最高 |
| h4 | Maroon | ~0.50 |
| h5 | Yellow | ~0.78 |
| h6 | Subtext1 | ~0.45 |

階層が下がるほど目立たなくなるべきだが、輝度が h1 → h3 → h5 で「中→**最高**→高」と
乱れていた。サイズが小さい間は気にならなかったが、コンポジターズスケールへの移行で
h3 が 18pt になった瞬間に問題が顕在化した。

**Lavender (`#b4befe`) を選んだ理由**:
1. Rosewater の「near-white」問題を解消（輝度 ~0.60）
2. h1 Mauve と purple 族で thematic continuity（warm purple → cool purple の echo）
3. mocha パレット中で未使用、衝突なし
4. latte 側も対称に変更（`#dc8a78` → `#7287fd`）して 2×2 lock-step を維持

latte 側は元の Rosewater (`#dc8a78`) が中位輝度で問題は無かったが、CLAUDE.md の lock-step
ルールに従って同時変更した（mocha のみ変更すると新しい差分軸が発生してしまう）。

### 採用しなかった代替

| 案 | 理由 |
|----|------|
| Sky `#89dceb` | Cool 一段が暖色 (Pink/Maroon) の中に入り temperature 不揃い |
| Peach `#fab387` | 輝度 ~0.70 で大して下がらない |
| Flamingo `#f2cdcd` | Rosewater と僅かしか違わない |
| Rosewater × bg blend | Catppuccin パレットを離れる、保守性低下 |

## Code blocks and inline raw

### Block

```typst
#show raw.where(block: true): it => block(
  fill: <surface>, inset: 12pt, radius: 6pt, width: 100%,
  text(font: <mono>, size: 10pt, it))
```

**10pt** は本文 12pt の約 0.83。Typst の `raw` ShowSet デフォルト `0.8em` (= 9.6pt) より
僅かに大きく、可読性を優先した値。

### Inline

```typst
#show raw.where(block: false): it => box(
  fill: <surface>, inset: (x: 0.3em / 0.8), outset: (y: 0.15em / 0.8), radius: 3pt,
  text(font: <mono>, size: 0.85em / 0.8, it))
```

`/0.8` 補正の経緯: Typst の `raw` ShowSet が `size: 0.8em` をデフォルト適用するため、
`box` 内側で再度 `0.85em` を指定すると 0.85 × 0.8 = 0.68em になってしまう。これを
打ち消すために `0.85em / 0.8 = 1.0625em` を指定し、実効サイズ 0.85em を実現している。
inset / outset の `/0.8` も同様の打ち消し。

将来 `show raw.where(block: false): set text(size: 1em / 0.8)` で一元化する選択肢が
ある（investigate §6.2）が、現状の inline 内 box 構造を維持するなら現方式で機能する。

## Lists, quotes, tables

### List markers

```typst
#set list(marker: ([•], [‣], [–]), indent: 1em, body-indent: 0.7em)
```

`•` (U+2022) → `‣` (U+2023) → `–` (U+2013) の 3 段階。Typst デフォルトの `•/-/*` より
階層感が出る選択。

### Indent

`indent: 1em` (マーカーまでのインデント) と `body-indent: 0.7em` (マーカーから本文)。
`body-indent` を 1em 未満に取るのは、`–` (`U+2013` en-dash) のグリフ幅を考慮した値
で、過剰に間延びしないようにしている。

### Block quote

```typst
#show quote.where(block: true): it => block(
  inset: (left: 16pt, y: 8pt),
  stroke: (left: 3pt + <accent>),
  text(fill: <muted>, it.body))
```

`16pt` 左 inset / `3pt` 左 stroke で「左罫」表現。GitHub-style block quote と類似。
本文色を muted (Subtext1 / Overlay 系) に変更してコンテンツの "二次性" を表現。

### Tables

```typst
#set table(stroke: 0.5pt + <line>, inset: 8pt,
  fill: (_, y) => if y == 0 { <surface> } else { none })
```

ヘッダ行のみ surface 色で塗り分け。罫は 0.5pt の hairline で「表組み感」を出しつつ
うるさくならない値。

## Math equations — 13pt

```typst
#show math.equation: set text(font: "STIX Two Math", size: 13pt)
```

本文 12pt に対して **13pt は意図的に大きめ**。STIX Two Math の x-height が本文フォント
より低めなため、同サイズだと数式が "小さく見える" 現象を補正する経験的調整。投稿で
12pt との A/B 候補を残しているが (investigate §1)、現状は 13pt 据え置き。

## Other text decorations

### Strong

```typst
#show strong: set text(fill: <body>)
```

色は本文と同じ、weight のみ変える。色変更による strong は Markdown の **強調** の
慣用と乖離するため、bold 単独で表現する。

### Emph

- **CJK テーマ**: `Teal` (mocha `#94e2d5` / latte `#179299`) で色変更
- **Latin テーマ**: 色指定なし → Fira Sans Italic にフォールバック

経緯: Noto Sans JP には italic style が無いため、CJK テーマで `*emph*` をイタリック化
できない。色変更が次善策。Teal を選んだのは「cool tones recede」の原理で、強調と言っても
italic は "subdued emphasis" であり、目を引きすぎるべきでないという判断。

圏点 (kenten) も検討したが不採用 (`../2026-03-20-experiment-cjk-emphasis.md`)。理由は
そちらを参照。

### Strike

```typst
#show strike: set strike(stroke: 1pt + <muted>)
```

線色を muted (Subtext1) に。本文色だと線が strong すぎ、消したことが視覚的に強すぎる。

### Link

```typst
#show link: it => text(fill: <accent-blue>, underline(it))
```

色 + underline。`underline(offset: 2pt)` で descender 回避を明示する選択肢があるが
(investigate §1)、Typst の `evade: true` がデフォルトで動くので現状未指定。

## OpenType features and language

### Settings

- `lang: "ja"` (CJK) / `lang: "en"` (Latin)
- `cjk-latin-spacing: auto` (CJK のみ、ただしデフォルト値なので明示は冗長)
- `kerning: true`, `ligatures: true` (Typst デフォルト、明示しない)

### `lang` の効果

- **Hyphenation dictionary** の選択（`lang: "en"` で英語ハイフン辞書）
- **Smart-quote** の方向 (`'` → `'` / `’` 等)
- **A11y metadata** (PDF 出力時)
- **kinsoku** (CJK 行頭/行末禁則) は **`lang` ではなく** Unicode line-break property +
  CJK script 検出が駆動するため、`lang: "ja"` の有無は禁則挙動に直接影響しない
  - ただし将来的な lang 連動への signaling として設定する価値はある

### `cjk-latin-spacing: auto`

和欧間に四分アキ (0.25em) を自動挿入。Typst デフォルトが `auto` なので明示しなくても
効くが、ドキュメント目的（「この設定を意識している」表明）として残している。

### 検討中の OpenType features

- CJK `palt` (proportional alternate widths): Noto Sans CJK JP では効くが、Google Fonts
  配信の Noto Sans JP subset では feature 搭載状況に履歴あり。`otfinfo -f` で実機検証
  した上で適用するか判断
- 数表での `tnum + lnum` (tabular figures, lining numerals): 表組み数値の桁揃えに有効。
  `table` 内のみで `text(number-width: "tabular")` を適用する形が候補

## Rejected experiments

過去に試して採用しなかった選択肢の記録 (詳細は各ドキュメント参照):

| 実験 | 結果 | 文献 |
|------|------|------|
| 圏点 (kenten) による CJK emph | 視認性は上がるが Markdown ビューアの role 過剰 | `../2026-03-20-experiment-cjk-emphasis.md` |
| Content column cap (frame width 制約) | 画面幅変化との相互作用が複雑 | `../2026-04-25-experiment-content-column-cap.md` |
| 純粋 modular scale (ratio 1.2) for headings | h1 = 29.9pt で過大 | 本ドキュメント |
| Heading 余白 baseline grid (流派 A) | 単段スクロール用途で利点が出ない | 本ドキュメント |
| Leading 1.0em (両テーマ共通) | slack に感じる、特に Latin 側 | 本ドキュメント (commit `e09a5eb`) |

## Open items

将来検討候補 (investigate §6):

1. **inline raw 補正 `/0.8` の `show raw: set text` への一元化** — 4 テーマ重複の解消
2. **`par(justification-limits)` 追加** (Typst 0.14.0 新機能) — word-space 肥大の緩和
3. **版面幅 narrow 実験** (580 → 540pt) — 規範的行長への寄せ
4. **tabular figures** (`text(number-width: "tabular")`) — 表組み数値整合
5. **link underline offset** (`offset: 2pt`) — descender 回避の明示化
6. **数式サイズの A/B** (12pt vs 13pt)
7. **`palt` for CJK** — Noto Sans JP のサブセットでの実機確認

## References

- 関連: `../2026-04-25-investigate-typst-typography.md` (調査・外部仕様の網羅評価)
- Bringhurst, *The Elements of Typographic Style* §2.1.2 (measure), §2.2.2 (vertical
  rhythm), §3.1 (compositor's scale)
- Butterick, [Practical Typography](https://practicaltypography.com/) — line-length /
  line-spacing / justified-text
- W3C [Requirements for Japanese Text Layout (JLReq)](https://www.w3.org/TR/jlreq/)
- [Typst 0.14 docs – text / par](https://typst.app/docs/reference/text/text/)
- [Catppuccin palette](https://github.com/catppuccin/catppuccin) — color role naming
