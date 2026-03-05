# mitex 変換の制約と回避策

mitex 0.2.4 + typst 0.14 環境での LaTeX → Typst 数式変換の制約をまとめる。

調査日: 2026-03-06
テストファイル: `tests/fixtures/10_math_showcase.md`

---

## 概要

mitex (`mitex::convert_math`) は LaTeX 数式を Typst 数式構文に変換するクレートだが、
対応範囲はかなり限定的。特に LaTeX 環境 (`\begin{...}`) 系と一部の頻出コマンドが
未対応で、変換後に Typst コンパイルエラーになるパターンが多い。

## 互換シムで解決済みのコマンド

`themes/mitex-compat.typ` で互換関数を定義し、以下のコマンドが動作するようになった。

### 環境系

| LaTeX | mitex 出力 | 互換シム | 結果 |
|-------|-----------|---------|------|
| `\begin{pmatrix}...\end{pmatrix}` | `pmatrix(...)` | `math.mat(delim: "(", ..)` | OK |
| `\begin{bmatrix}...\end{bmatrix}` | `bmatrix(...)` | `math.mat(delim: "[", ..)` | OK |
| `\begin{vmatrix}...\end{vmatrix}` | `vmatrix(...)` | `math.mat(delim: "\|", ..)` | OK |
| `\begin{Vmatrix}...\end{Vmatrix}` | `Vmatrix(...)` | `math.mat(delim: "‖", ..)` | OK |
| `\begin{matrix}...\end{matrix}` | `matrix(...)` | `math.mat(..)` | OK |
| `\begin{array}{cc}...\end{array}` | `mitexarray(...)` | `math.mat(..)` | OK（基本対応） |
| `\begin{cases}...\end{cases}` | `cases(...)` | Typst 組み込み | OK |
| `\begin{align}...\end{align}` | 未検証 | — | — |

### テキスト・演算子系

| LaTeX | mitex 出力 | 互換シム | 結果 |
|-------|-----------|---------|------|
| `\operatorname{sgn}` | `operatorname(s g n)` | `math.op(..)` | OK |
| `\text{sgn}` | `#textmath[sgn];` | identity 関数 | OK |
| `\mathbf{u}` | `mitexmathbf(u)` | `math.bold(math.upright(..))` | OK |
| `\mathbb{R}` | `RR` | Typst 組み込み | OK |
| `\mathrm{...}` | `upright(...)` | Typst 組み込み | OK |

### 関数系

| LaTeX | mitex 出力 | 互換シム | 結果 |
|-------|-----------|---------|------|
| `\sqrt{x}` | `mitexsqrt(x)` | `math.sqrt(..)` | OK |
| `\sqrt[n]{x}` | `mitexsqrt([n], x)` | `math.root(..)` | OK |
| `\pmod{p}` | `pmod(p)` | `(mod p)` | OK |
| `\overbrace{...}` | `mitexoverbrace(...)` | `math.overbrace(..)` | OK |
| `\underbrace{...}` | `mitexunderbrace(...)` | `math.underbrace(..)` | OK |
| `\displaystyle` | `mitexdisplay(...)` | `math.display(..)` | OK |

## 未対応コマンド一覧

### Typst 0.14 非推奨警告（mitex が古い構文を出力）

| mitex 出力 | 推奨 | 備考 |
|-----------|------|------|
| `diff` | `partial` | `\partial` の変換結果 |
| `planck.reduce` | `planck` | `\hbar` の変換結果 |
| `angle.l` / `angle.r` | `chevron.l` / `chevron.r` | `\langle` / `\rangle` の変換結果 |

これらは warning であり動作はするが、将来の typst で削除される可能性がある。

## 動作確認済みコマンド

以下は問題なく変換される:

- 基本演算: `\frac`, `\sum`, `\prod`, `\int`, `\iint`, `\oint`, `\lim`
- 上付き・下付き: `^{}`, `_{}`
- ギリシャ文字: `\alpha`, `\beta`, `\pi`, `\mu`, `\sigma`, `\lambda`, `\theta`, `\varepsilon` 等
- 記号: `\cdot`, `\cdots`, `\ldots`, `\leq`, `\geq`, `\equiv`, `\to`, `\infty`, `\nabla`, `\times`
- 括弧: `\left(`, `\right)`, `\left[`, `\right]`, `\left|`, `\right|`
- 装飾: `\bar{X}`, `\hat{x}`（一部）
- フォント: `\mathbb{R}` → `RR`
- 空白: `\,` (thin), `\quad`
- アクセント: `'` (prime)
- 関数名: `\det`, `\sin`, `\cos`, `\tan`, `\exp`, `\ln`, `\log`, `\gcd`, `\mod`

## 回避策

行列・平方根・演算子・テキスト等は `themes/mitex-compat.typ` の互換シムにより解決済み。

## 影響範囲

数式ショーケース (`10_math_showcase.md`) では行列・平方根・演算子等を含む幅広い数式が動作する。
`themes/mitex-compat.typ` の互換シムにより、mitex crate 単体での実用性が大幅に向上した。

## 今後の方針

- [x] ~~mitex 出力の後処理層~~ → `themes/mitex-compat.typ` で Typst 側の互換シムとして実装済み
- [ ] `\partial` → `partial`（非推奨 `diff` 回避）の後処理を `convert.rs` に追加
- [ ] mitex upstream の issue/PR を確認して対応状況をウォッチ
