了解です。リサーチ結果を要約版で提示します。

---

# Typst 組版設定レビュー（要約版）

**対象**: mlux の現行テーマ (`themes/catppuccin*.typ`)
**前提**: 本文 12pt / 版面幅 660pt(内容幅 580pt) / Noto Sans JP または Fira Sans / leading 0.75em / `justify: true` / ragged では no-hyphen
**独立検証日**: 2026-04-25 — Typst 0.14 API 仕様、Bringhurst / Butterick / JLReq の数値、upstream issue の状態を並列で再検証し、本稿を差し替え。

## 0. 適用済み（2026-04-25 時点）

初稿で「高優先」に挙げた 3 項目は既に HEAD に反映済み:

| 項目 | 適用コミット | 備考 |
|---|---|---|
| leading `1em` → `0.75em` | `e09a5eb` | 行間 2em → 1.75em (総 line-height ≈ 175% of font-size) |
| `cjk-latin-spacing: auto` | `3307a46` | 検証済み: これは `text` の **デフォルト**なので明示設定は冗長（害は無い、ドキュメント目的として残してよい） |
| `lang: "ja"` / `lang: "en"` | `3307a46` | `lang` 単独では kinsoku は駆動されない (後述 §3) ＝ 実効は英語ハイフネーション選択と smart-quote |
| `linebreaks: "optimized"` 明示不要 | `3307a46` 注記 | 再検証 2026-04-25: HEAD で `linebreaks` を追加/非追加の 2 ビルドを byte-compare → SHA256 一致 (`c7e5db02…`)。`justify: true` のとき `linebreaks: auto` は `"optimized"` に解決される |

## 1. 残存する乖離箇所（影響大→小）

現行テーマ (`catppuccin.typ` / `catppuccin-latin.typ` / `catppuccin-latte.typ` / `catppuccin-latte-latin.typ`) との差分のみ。

| 項目 | 現状 | ベストプラクティス | 優先 |
|---|---|---|---|
| **measure (欧文)** | 580pt / 12pt ≈ 96 cpl | Bringhurst 45-75（66 理想）、Butterick 45-90、Bringhurst 30×font-size ルールで上限 40× = 480pt | 高 |
| **行長 (和文)** | 580pt / 12pt ≈ 48 全角字 | JLReq: **横組み 40 字以下推奨**（縦組みは 52 字以下）— 現行は横組み上限超過 | 高 |
| **見出し modular scale** | 24/20/16/14/13/12（比 1.200/1.250/1.143/1.077/1.083） | 単一比 1.2 なら 12/14.4/17.3/20.7/24.9 / 1.25 なら 12/15/18.75/23.44/29.3 | 中 |
| **見出し余白 (mocha)** | `catppuccin.typ` / `catppuccin-latin.typ` は全 level 一律 above:1.4em, below:0.9em | 本文 leading の整数倍 + above:below は asymmetric（例 3:1 や 2:1）で Gestalt proximity を作る | 中 |
| **見出し余白 (latte)** | `catppuccin-latte*.typ` は 2.2/1.9/1.6/1.3/1.2/1.0em（テーパー済） | leading = 0.75em × 12pt = 9pt へ整数倍でスナップするとより良い | 中 |
| **inline raw の `/0.8` 補正** | 3 テーマ全てで手書き | `show raw.where(block: false): set text(size: 1em / 0.8)` で一元化（Typst docs の推奨イディオム） | 中 |
| **link underline offset** | デフォルト（`evade: true` で descender は自動回避済み） | 読みやすさ志向なら `offset: 2pt` を明示 | 低 |
| **数式サイズ** | 13pt（STIX Two Math） | 12pt でも STIX の x-height が高いため本文と整合する可能性 → A/B 推奨 | 低 |
| **tabular 数字** | 未指定 | `table` 内でのみ `text(number-width: "tabular")` 推奨（「numbers」ではない） | 低 |
| **OpenType features** | kerning / ligature のみ既定 | CJK: `palt`（Noto Sans CJK JP で効く、Google Fonts 配信の Noto Sans JP サブセットは有無の履歴あり）／欧文表で `tnum + lnum` | 低 |
| **`par.justification-limits` (0.14 新機能)** | 未使用 | `(tracking: (min: 0pt, max: 0.04em))` で文字追い込み預金を増やし、justify の白スペース肥大を緩和 | 低 |

（emph と h2 色の重複は解消済: `catppuccin.typ` の emph は Teal `#94e2d5`, `catppuccin-latte.typ` は Teal `#179299`。Latin テーマは emph 色指定なし＝ Fira Sans italic デフォルト。初稿の「両方 Pink」は stale）

## 2. 欧文タイポ原則（検証済）

- **measure**: Bringhurst は 45-75（66 字を理想）、Butterick は 45-90 で設定。580pt / 12pt = 96 cpl は両者の上限を超過。解決は (a) 版面幅を 480-540pt に狭める、(b) 本文を 11pt に縮める、(c) マルチカラム化のいずれか（出典: [Practical Typography – line-length](https://practicaltypography.com/line-length.html)、Bringhurst §2.1.2）。
- **leading**: Butterick の 120-145% は **総 line-height** 基準。Typst `par.leading` はギャップ（下端↔上端）のみであり、概ね line-box ≈ 1em + gap = 総 line-height。つまり `leading: 0.5em` ≈ 総 150%、`leading: 0.75em` ≈ 総 175% に相当する。現行 0.75em は Butterick 上限（145%）をわずかに上回るが、スクロール UX を考慮すれば許容範囲（出典: [Practical Typography – line-spacing](https://practicaltypography.com/line-spacing.html)、[Typst docs – par](https://typst.app/docs/reference/model/par/)）。
- **vertical rhythm**: 見出し余白の合計は本文 leading の整数倍。Bringhurst は asymmetric（例 1.5 + 0.5 lines）を明示的に許容。原理は Gestalt proximity — 見出しは後続コンテンツとグループ化すべきなので above > below（比 2:1 は慣習、規範ではない）。出典: [webtypography.net §2.2.2](http://webtypography.net/2.2.2)。
- **modular scale**: 比 1.2 / 1.25 / 1.333 / 1.414 / 1.5 / 1.618 が定番。Spencer Mortensen 公式 `f_i = f_0 · r^(i/n)` が対数等間隔を保証（出典: [The typographic scale](https://www.spencermortensen.com/articles/typographic-scale/)）。
- **justification**: Butterick「justify するならハイフネーション必須」は canonical。cpl 下限は Butterick 自身は言及無し — 40-45 cpl + hyphen / 60 cpl なしが実務 folklore。

## 3. Typst 0.14 の knob（検証済）

### 既に使える（安定）
- `text(cjk-latin-spacing: auto)`: 0.25em の四分アキを和欧間に挿入。**デフォルトは `auto`** なので明示は冗長。出典: [Typst docs / forum](https://forum.typst.app/t/unicode-drafts-a-report-on-text-cjk-latin-spacing/2446)
- `par(linebreaks: "optimized")`: `justify: true` のとき `auto` の解決先と同一 → **no-op**（独立検証で byte-identical を確認）
- `text(costs: (hyphenation, runt, widow, orphan))`: 4 種のコストキーすべて存在。`costs` は `text` にある（`par` ではない）
- `text(overhang: true)`: デフォルト on。**2026-04 時点の 0.14 で実装されているのは右側のみ**（左側 burasage は PR #7782 で進行中、未マージ）
- `text(features: ("palt",))` ≡ `(palt: 1)`: フォント側の GSUB 依存
- **`par(justification-limits: ...)` (0.14.0 新機能)**: 文字レベル justify（tracking / spacing の min-max を dict で）。日本語で word-space 肥大する場面の緩和に有効。デフォルトは `(spacing: (min: 66.67%, max: 150%), tracking: (min: 0pt, max: 0pt))`
- `show raw.where(block: false): set text(size: 1em / 0.8)`: Typst docs 自身が示すイディオム。デフォルト 0.8em を打ち消す

### 訂正が入った前提
- **`lang: "ja"` と kinsoku の関係**: 行頭/行末禁則は **Unicode line-break property + CJK script 検出** が実装の根拠であり、`lang` 単独は駆動しない。`lang` の主効果は hyphenation dictionary / smart quote / a11y metadata。ただし `lang: "ja"` を設定する価値はある（将来的に挙動が lang 連動することへの signaling、および smart-quote 適正化）
- **サブスクリプト/スーパースクリプト**: 0.14 で Unicode codepoint から OT `subs`/`sups` へ実装変更。数式外の `_` / `^` 表記の品質が上がっている

### Typst に無い／不完全
- **左側 burasage**（行頭 `「` 等を margin へ）: [#7231 open](https://github.com/typst/typst/issues/7231) — PR #7782 （2026-04-17 active）で両マージン対応中
- **約物追い込み / 追い出し**: [#6582 open](https://github.com/typst/typst/issues/6582) — 上記 PR #7782 と連動
- **`raw` 境界の CJK-Latin アキ欠損**: [#2702 open](https://github.com/typst/typst/issues/2702)（stale）
- **inline equation 境界の CJK-Latin アキ欠損**: [#2703 open](https://github.com/typst/typst/issues/2703)
- **`$...$` 内の stylistic-set**: [#1431 は 2023-07 closed だが完全解消ではない](https://github.com/typst/typst/issues/1431) → 関連 [#5850](https://github.com/typst/typst/issues/5850) で再発。workaround: math font を **配列**で渡す (`font: ("New Computer Modern Math",)`)

### main に merged・未 release（次バージョンに載る予定）
- [#7606 Fix uneven CJK-Latin spacing in justified paragraphs](https://github.com/typst/typst/pull/7606) (merged 2026-01-20)
- [#7662 overhang=false in math `layout_inline_text`](https://github.com/typst/typst/pull/7662) (merged 2026-01-08)

現行 release は 0.14.2 (2025-12-12) で、上記 2 件はその後の merge のため未同梱。次 release でアップデート可能性あり。

## 4. 和文特有（JLReq）

- **行間 (gap)**: JLReq は字幅（全角）の **50-100%**（leading 0.5em-1em）を推奨。短い行長では 0.5em、長い行長（35 字超）では 1em 寄り。現行 0.75em は行長 48 字に対して妥当。出典: [W3C JLReq](https://www.w3.org/TR/jlreq/)
- **行長**: 横組みは **40 字以下** / 縦組みは 52 字以下。現行 48 字全角は **横組み上限超過**（初稿の「35-52 推奨中央」は縦組み規定との混同）。mlux はスクロール型ビューアで version-specific な UX を持つので、これはトレードオフ（紙組み規範では narrow 推奨、画面縦スクロールでは水平方向のコンテキスト温存が価値を持つ）
- **フォント選定**: Typst の font fallback は character-by-character（出典: [Typst text docs](https://typst.app/docs/reference/text/text/)）。`font: ("Fira Sans", "Noto Sans JP")` は欧文 Fira / 和文 Noto の組合せとして機能する
- **Noto Sans JP の OT features**: 「Noto Sans CJK JP」（Adobe/Google フルビルド、Source Han Sans 派生）と「Noto Sans JP」（Google Fonts 配信 subset）で feature 搭載状況が異なる。`palt` は前者で安定搭載、後者は除去/復活の履歴あり。`otfinfo -f <font.ttf>` で実機確認が確実
- **強調**: 圏点は実験済み不採用（`docs/2026-03-20-experiment-cjk-emphasis.md`）。色変更が妥当。emph 色は既に Teal 系へ移行済

## 5. 推奨 Typst スニペット（次ステップ）

現行 HEAD を前提に、§1 の残課題を段階的に潰す形のスニペット:

```typst
// 本文（共通）
#set text(
  size: 12pt,
  kerning: true,
  ligatures: true,
  // cjk-latin-spacing: auto は既定なので省略可能
)
#set par(
  leading: 0.75em,
  justify: true,
  first-line-indent: 0pt,
  // 新: 文字レベル justify で word-space 肥大を緩和
  justification-limits: (tracking: (min: 0pt, max: 0.04em)),
)

// 見出し (modular scale 1.2 ベース)
// 本文 line-height ≈ 12pt + 0.75em×12pt = 21pt を 1 単位として、
// above:below = 2:1（heading は後続コンテンツ寄り）
// 注: block() の above/below の em 解決は surrounding/heading のどちらか
// 検証が必要。確実性を優先するなら pt で書く: above: 42pt, below: 21pt
#show heading: it => block(above: 42pt, below: 21pt, it)
#show heading.where(level: 1): set text(24.9pt, weight: "bold")
#show heading.where(level: 2): set text(20.7pt, weight: "bold")
#show heading.where(level: 3): set text(17.3pt, weight: "bold")
#show heading.where(level: 4): set text(14.4pt, weight: "bold")
#show heading.where(level: 5): set text(12pt,   weight: "bold")
// h6 は本文と同サイズなので太字 + 色で区別

// raw の 0.8em 補正を一元化
#show raw.where(block: false): set text(size: 1em / 0.8)

// リンク（descender を視覚的に回避したい場合のみ）
#show link: it => text(fill: ..., underline(offset: 2pt, it))

// 版面幅を読みやすさへ寄せる選択肢
// (a) 現行維持 + UX 優先
// (b) 版面 540pt 程度へ narrow（96 cpl → 90 cpl 圏内）
// (c) 本文 11pt に縮める（96 cpl → 106 cpl → 悪化、非推奨）

// CJK テーマ（kinsoku は Unicode LB が駆動、lang は signaling）
#set text(font: ("Noto Sans JP",), lang: "ja")

// Latin テーマ
#set text(font: ("Fira Sans",), lang: "en",
  costs: (hyphenation: 150%))  // 狭い版面で hyphen 抑制したい場合
```

## 6. 次の実験順序（未実施のもの）

1. **見出し余白 & modular scale 再設計**: `catppuccin.typ` / `catppuccin-latin.typ` は level 一律 1.4em のため最も効果大。latte 版を先にベンチとして比較可能
2. **inline raw の `/0.8` を `show raw: set text` へ**: 4 テーマで重複している 3 行がそれぞれ 1 行に減る。リファクタ
3. **`par(justification-limits)` 追加**: word-space 肥大の実サンプルで A/B。改善が見えない場面では省略
4. **版面幅 narrow の実験**: 580 → 540pt（80 cpl 圏内に収める）。和欧両方の可読性を改善するが、スクロール UX と tile サイズとの相互作用を確認必要
5. **tabular 数字** / **underline offset**: micro-optimization。最後

## 7. 未解決（upstream 依存）

| 項目 | 追跡 | 現状 |
|---|---|---|
| 左側 burasage | [#7231](https://github.com/typst/typst/issues/7231), [#6582](https://github.com/typst/typst/issues/6582) | PR [#7782](https://github.com/typst/typst/pull/7782) active（2026-04-17） |
| raw / math 境界の和欧アキ | [#2702](https://github.com/typst/typst/issues/2702), [#2703](https://github.com/typst/typst/issues/2703) | open、進展なし |
| math 内 stylistic-set | [#1431 closed](https://github.com/typst/typst/issues/1431) / [#5850](https://github.com/typst/typst/issues/5850) | font 配列 workaround 存在 |
| CJK justify 時の和欧アキ | [#7606 merged 2026-01](https://github.com/typst/typst/pull/7606) | 0.14.2 未同梱、次 release 待ち |
| 数式行内の overhang | [#7662 merged 2026-01](https://github.com/typst/typst/pull/7662) | 同上 |
| フレーズ単位の CJK 改行 (`word-break: auto-phrase`) | [#8097 open](https://github.com/typst/typst/issues/8097) | 新規提案（2026-04） |
| Noto Sans JP italic なし | font の仕様 | emph の italic 化は Latin テーマでのみ有効（CJK テーマは色変更で代替） |

## Appendix A. 実サイトとの比較 — GitHub の line-height

レビュー中の議論で「0.75em は詰まって感じる」という感覚検証のため、実運用サイトの値を確認したメモ。

### GitHub.com の本文

ブラウザの DevTools で `<body>` の計算済みスタイルを確認:

```
font-size:   16px
line-height: 24px
→ 総 line-height = 24/16 = 1.5em (150%)
→ Typst 換算: par.leading ≈ 0.5em
```

CSS の `line-height` はベースライン間距離そのもの、Typst の `par.leading` は line-box 間のギャップ。ほとんどのフォントで line-box ≈ 1em なので `leading = 総 − 1em` が近似的に成り立つ（±0.05em 程度のフォント metrics 依存あり）。

### 既知の基準と並べた位置付け

| 基準/実装 | 総 line-height | Typst leading 換算 |
|---|---|---|
| Butterick 推奨下限 | 120% | 0.2em |
| Butterick 推奨上限 | 145% | 0.45em |
| **GitHub 本文** | **150%** | **0.5em** |
| Typst `par.leading` デフォルト | ~165% | 0.65em |
| **mlux 現行** | **~175%** | **0.75em** |
| JLReq 中央値 (gap 75%) | 175% | 0.75em |
| JLReq 上限 (gap 100%, 長い行) | 200% | 1em |
| mlux 旧設定 (2026-04 以前) | ~200% | 1em |

### 解釈

- **GitHub の 150% は Latin 前提の値**。system-ui スタック (`-apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans", Helvetica, Arial, sans-serif`) は和文フォントに最適化されておらず、日本語表示時に詰まって見える問題は広く知られている。
- **mlux を GitHub と同じ 150% にするのは CJK 主体では過小**。JLReq 下限 (150%) は「短い行長 (~30 字)」向けで、現行 48 字の行長には不足。
- **Latin テーマ** (`catppuccin-latin.typ` / `catppuccin-latte-latin.typ`) **を GitHub-like の 0.5em へ下げる選択肢は合理的**。Butterick 上限 (145%) の直上で、Markdown ビューアの典型的なコンテンツ密度にも適合する。CJK テーマと分離する方針の一部として検討余地あり。
- **§4 の JLReq 観点**とあわせると、CJK テーマは **据え置き or 0.85em へ引き上げ**、Latin テーマは **0.5em 前後へ引き下げ** が素直なデザイン分岐。現行は 4 テーマ共通 0.75em で Latin/CJK 規範の交点を狙った妥協値という位置付け。

### CSS/Typst 単位系のズレに関する注意

- CSS `line-height: 1.5` は厳密に baseline-to-baseline = 1.5 × font-size
- Typst `par.leading: 0.5em` は line-box 間ギャップ。line-box は font の ascender + descender 由来で、Noto Sans JP / Fira Sans とも概ね 1em だが厳密には ±0.05em 程度ぶれる
- 変換式 `leading ≈ total − 1em` は実用近似。pixel-perfect な一致が要るなら実測すること

## 参考文献

- [Butterick – Practical Typography: line-length](https://practicaltypography.com/line-length.html)、[line-spacing](https://practicaltypography.com/line-spacing.html)、[justified-text](https://practicaltypography.com/justified-text.html)
- [webtypography.net – Bringhurst §2.2.2 vertical rhythm](http://webtypography.net/2.2.2)
- [W3C – Requirements for Japanese Text Layout (JLReq)](https://www.w3.org/TR/jlreq/)
- [Typst 0.14 docs – text / par](https://typst.app/docs/reference/text/text/)
- [Typst 0.14.0 changelog (justification-limits など)](https://typst.app/docs/changelog/0.14.0/)
- [Typst Issue #276 – Better CJK support (umbrella)](https://github.com/typst/typst/issues/276)
- [Spencer Mortensen – The typographic scale](https://www.spencermortensen.com/articles/typographic-scale/)
- [notofonts/noto-cjk リポジトリ](https://github.com/notofonts/noto-cjk)
