了解です。リサーチ結果を要約版で提示します。

---

# Typst 組版設定レビュー（要約版）

**対象**: mlux の現行テーマ (`themes/catppuccin*.typ`)
**前提**: 本文 12pt / 版面幅 660pt(内 580pt) / Noto Sans JP または Fira Sans / leading 1em / ragged-right no-hyphen

## 1. 現状の乖離箇所（影響大→小）

| 項目 | 現状 | ベストプラクティス | 優先 |
|---|---|---|---|
| **leading** | `1em`（行送り 200%） | 120-145%（欧文）/ 150-180%（和文）| 高 |
| **measure (欧文)** | ~96 cpl | 45-75 cpl 理想、最大 90 | 高 |
| **和欧混植** | `cjk-latin-spacing` 未設定 | `auto` 必須 | 高 |
| **`lang: "ja"`** | 未設定 | 設定で禁則処理が有効化 | 高 |
| **justify / linebreaks** | `justify: true` のみ | `linebreaks: "optimized"` 併用（Knuth-Plass）| 中 |
| **見出し modular scale** | 24/20/16/14/13/12 は比率不均一 | 単一比（1.2 か 1.25）| 中 |
| **見出し余白** | 自サイズの em | 本文 leading の整数倍（vertical rhythm）| 中 |
| **inline raw の `/0.8` 補正** | 場当たり的 | `show raw: set text(size: 1em/0.8)` で一元化 | 中 |
| **link underline offset** | デフォルト（ディセンダに食い込む）| `offset: 2pt` | 低 |
| **emph と h2 色の重複** | 両方 Pink `#f5c2e7` | Lavender 系へ | 低 |
| **数式サイズ** | 13pt（本文より大）| 12pt（x-height が STIX は高い）| 低 |
| **tabular 数字** | 未指定 | 表は `number-width: "tabular"` | 低 |
| **OpenType** | kerning のみ既定 | liga/ss02 など明示 | 低 |

## 2. 欧文タイポ原則（Bringhurst / Butterick）

- **measure**: 66 字が理想、45-90 が許容。現行 96cpl は上限超過。解決策は (a) 版面幅を 520pt に狭める / (b) 本文を 14pt に拡大。
- **leading**: 字サイズの 120-145%。12pt→14.4-17.4pt。現行 24pt は「空きすぎ」で行の連続性を損なう。
- **vertical rhythm**: 見出しの above/below は本文 leading の整数倍。above:below ≈ 2:1（見出しは後続コンテンツにグループ化すべき）。
- **modular scale**: 比 1.2（minor third）なら 12/14.4/17.3/20.7/24.9/29.9。
- **justification**: 45cpl 以上 + ハイフネーション有効が前提。片方欠けるとリバー発生。
- **微修飾**: 見出しは `hyphenate: false`、本文で `liga: on`、表で `tnum + lnum`、散文で `onum`（ただし Fira Sans には onum 無し→no-op）。

## 3. Typst 0.14 の主要機能

### 既に使える knob
- `text(cjk-latin-spacing: auto)`: 和欧間に四分アキ自動挿入（JLReq §3.8 準拠の近似）
- `par(linebreaks: "optimized")`: Knuth-Plass。ragged でも行長が揃う
- `text(costs: (hyphenation: 150%))`: ハイフネーション抑制
- `text(overhang: true)`: **右側のみ** ぶら下げ（デフォルト on）
- `text(features: ("palt",))`: 約物詰め（Noto Sans JP 搭載）
- 基本禁則: `lang: "ja"` で自動有効（「『【 は行頭禁止、。、」』 は行末禁止）

### Typst に無い／不完全
- **左側ぶら下げ**（行頭の `「` 等をマージンへ）: Issue #7231 open
- **約物の burasage / 追い込み・追い出し**: Issue #6582 open
- **`raw` / inline equation 境界での CJK-Latin 空き**: Issue #2702, #2703（部分修正あり、一部残存）
- **Math 内の stylistic-set**: Issue #1431（`$...$` 内で効かないケース）

## 4. 和文特有（JLReq）

- **行間**: 字幅の 50-100%（leading 0.5em-1em）が和文本文の推奨域。現行 1em は上限ギリギリだが **スクロール型ビューアの UX には合致**。読み物用に 0.75em 切替を CLI 化する余地あり。
- **行長**: 580pt÷12pt ≈ 48 全角字。JLReq 推奨 35-52 字 の中央付近で **適正**（欧文換算の cpl 超過問題とは別物）。
- **フォント選定**: Noto Sans JP を維持。`font: ("Fira Sans", "Noto Sans JP")` の順序は正しい（文字単位フォールバックなので欧文は Fira、和字は Noto が選ばれる）。
- **強調**: 圏点は実験済み不採用（`docs/2026-03-20-experiment-cjk-emphasis.md`）。色変更継続が妥当。ただし emph 色と h2 色の重複は要修正。

## 5. 推奨 Typst スニペット（改善後イメージ）

```typst
// 共通
#set text(
  size: 12pt,
  kerning: true,
  ligatures: true,
  cjk-latin-spacing: auto,
)
#set par(
  leading: 0.75em,           // 1em → 0.75em（行送り 175%）
  spacing: 1.2em,
  justify: true,
  linebreaks: "optimized",   // Knuth-Plass
  first-line-indent: 0pt,
)

// 見出し（modular scale 1.2 ベース、余白は本文 leading の倍数に）
#show heading: it => block(above: 1.75em, below: 0.875em, text(weight: "bold", it.body))
#show heading.where(level: 1): set text(24pt, fill: rgb("#cba6f7"))
// ... level 2-6 も set text のみに簡略化

// raw の 0.8em 補正を一元化
#show raw.where(block: true): set text(size: 1em / 0.8)
#show raw.where(block: false): it => box(
  fill: ..., inset: (x: 0.3em), outset: (y: 0.15em), radius: 3pt, it)

// リンク（ディセンダ回避）
#show link: it => text(fill: ..., underline(offset: 2pt, it))

// CJK テーマのみ
#set text(font: ("Noto Sans JP",), lang: "ja")

// Latin テーマのみ
#set text(font: ("Fira Sans",), lang: "en",
  costs: (hyphenation: 150%))  // 580pt 幅なのでハイフン抑制
```

## 6. 推奨される実験順序

1. **leading 変更** (`1em` → `0.75em`): 最も効果が大きく、低リスク。A/B で見比べるのが容易
2. **`cjk-latin-spacing: auto` + `lang: "ja"`** 追加: 和文 README のビジュアルが顕著に変わる
3. **`linebreaks: "optimized"`**: ragged でも行端の凹凸が減る
4. **modular scale 見直し** (24/20/16/14/13/12 → 1.2 比 or 1.25 比で再設計)
5. **inline raw の `/0.8` 補正を show-set へ**: コード重複が消える
6. **emph 色変更 (h2 衝突回避)**: 1 行の機械的変更
7. **features/tnum 等の microtypography**: 最後

## 7. 未解決（upstream 依存）

- 左側ぶら下げ / 和文 burasage (約物の追い込み)
- raw / 数式境界の和欧スペース欠損（一部残存）
- Math 内 stylistic-set 反映問題
- Noto Sans JP 自体にイタリック無 → emph の italic 化は latin-mode のみ有効

## 参考文献（主要なもののみ）

- [Butterick – Practical Typography](https://practicaltypography.com/) (line-length / line-spacing / hyphenation の各章)
- [webtypography.net – Bringhurst applied to the web](http://webtypography.net/)
- [W3C – Requirements for Japanese Text Layout (JLReq)](https://www.w3.org/International/jlreq/)
- [Typst 0.14 docs – text/par/heading/raw/smartquote](https://typst.app/docs/reference/text/text/)
- [Typst Issue #276 – Better CJK support](https://github.com/typst/typst/issues/276)
- [Typst Issue #7231 – Hanging punctuation (left)](https://github.com/typst/typst/issues/7231)
- [Spencer Mortensen – The typographic scale](https://spencermortensen.com/articles/typographic-scale/)

