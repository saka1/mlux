# Latin Mode Font & Theme Design

## Overview

latin-mode のフォントとテーマ設定を最適化する。本文フォントを Fira Sans、
コードフォントを Fira Mono に統一し、emph をイタリック体に切り替える。
また、Fira + STIX Two Math を `cargo publish` に含める。

## Background

latin-mode（PR #15）により、CJK文字を含まない文書は自動的に `-latin` テーマ
バリアントを使用するようになった。しかしフォントは Noto Sans JP のままで、
emph も色（teal）による代替表現のままだった。

latin-only 文書では:
- バイナリサイズの制約が緩い（ラテンフォントは CJK フォントより桁違いに小さい）
- 真のイタリック体が使える
- 本文とコードのフォントファミリーを統一できる

## Target use case

技術ドキュメント / README（コードブロック多め、見出し・リスト中心）。

## Design decisions

### Font selection: Fira Sans + Fira Mono

**本文**: Fira Sans（Mozilla 設計のヒューマニストサンセリフ、OFL 1.1）
- Regular, Bold, Italic, BoldItalic の 4 ウェイト

**コード**: Fira Mono（リガチャなし）
- Regular, Bold の 2 ウェイト
- Fira Code ではなく Fira Mono を採用。理由: mlux はドキュメントビューアであり、
  `!=` → `≠` 等のリガチャ変換は著者の意図した表現と乖離する可能性がある

**数式**: STIX Two Math（変更なし）

**却下した候補:**
- Inter + DejaVu Sans Mono: 可読性は高いが本文とコードのデザイン系統が異なる
- Inter + Fira Code: ベストオブブリードだがファミリー不統一
- Fira Sans + Fira Code: リガチャがドキュメントビューアには過剰

### Emphasis: color → italic

CJK テーマの `#show emph: set text(fill: teal)` は、日本語フォントにイタリック
バリアントがないことへのワークアラウンドだった。

latin テーマではこの show rule を削除する。Typst はデフォルトで emph にイタリックを
適用するため、Fira Sans Italic が自動選択される。

CJK テーマ側の emph 色ルールは変更しない。

### Font sizes: no change

現行値（本文 12pt、コード 10pt、見出し 24/20/16/14/13/12pt、数式 13pt）を
そのまま維持する。Fira Sans のメトリクスは Noto Sans JP と大きく乖離していない。
微調整が必要なら後続 PR で対応。

### cargo publish inclusion

**問題**: 現状 `fonts/` がまるごと `cargo publish` から除外されており、
`cargo install mlux` ではフォントが埋め込まれない。latin テーマで `"Fira Sans"`
を指定しても、システムに Fira がなければフォールバック先が不定になる。

**解決**: Noto Sans JP（10.6MB）のみ除外し、Fira + STIX を同梱する。

```toml
# Cargo.toml
exclude = ["docs/", "fuzz/", "tests/", "fonts/NotoSansJP-*.ttf", "CLAUDE.md"]
```

crate サイズ試算: Fira 6 ファイル (~2.6MB) + STIX (~1.5MB) + ソース = 10MB 以内。

| Build source | Embedded fonts | CJK | Latin | Math |
|-------------|---------------|-----|-------|------|
| git clone (local) | Noto Sans JP + Fira + STIX | Full | Full | Full |
| `cargo install` (crates.io) | Fira + STIX only | System fallback | Full | Full |

## Embedded font files

| File | Purpose | ~Size |
|------|---------|-------|
| `fonts/FiraSans-Regular.ttf` | Body text | 500KB |
| `fonts/FiraSans-Bold.ttf` | Bold text | 500KB |
| `fonts/FiraSans-Italic.ttf` | Emphasis | 500KB |
| `fonts/FiraSans-BoldItalic.ttf` | Bold emphasis | 500KB |
| `fonts/FiraMono-Regular.ttf` | Code blocks | 300KB |
| `fonts/FiraMono-Bold.ttf` | Code blocks bold | 300KB |
| `fonts/OFL-FiraSans.txt` | License | - |
| `fonts/OFL-FiraMono.txt` | License | - |

Total: ~2.6MB (pre-compression). zstd-compressed at build time by `build.rs`.

## Theme file changes

### `catppuccin-latin.typ` / `catppuccin-latte-latin.typ`

Changes:
1. `#set text(font: "Fira Sans", ...)` (was `"Noto Sans JP"`)
2. Code block font: `text(font: "Fira Mono", ...)` (was `"DejaVu Sans Mono"`)
3. Inline code font: same change
4. Delete `#show emph: set text(fill: rgb("..."))` line

No changes to: headings, quotes, tables, links, strong, strikethrough, math, HR,
image placeholder, syntax highlight theme.

### CJK themes (`catppuccin.typ`, `catppuccin-latte.typ`)

No changes.

## Files changed

| File | Change |
|------|--------|
| `fonts/FiraSans-*.ttf` | New: 4 font files |
| `fonts/FiraMono-*.ttf` | New: 2 font files |
| `fonts/OFL-FiraSans.txt` | New: license |
| `fonts/OFL-FiraMono.txt` | New: license |
| `themes/catppuccin-latin.typ` | Font names + emph rule |
| `themes/catppuccin-latte-latin.typ` | Font names + emph rule |
| `Cargo.toml` | `exclude` pattern change |

## Files NOT changed

- `build.rs` -- auto-detects `.ttf` files in `fonts/`
- `src/theme.rs` -- ThemeEntry definitions unchanged
- `src/pipeline/` -- no pipeline changes
- `themes/catppuccin.typ`, `themes/catppuccin-latte.typ` -- CJK themes untouched
- Tests -- existing tests run in CJK mode, unaffected

## Out of scope (future PRs)

- Font size fine-tuning after visual inspection
- Typography micro-adjustments (`par(leading: ...)`, letter-spacing, etc.)
- Latin-mode integration tests
