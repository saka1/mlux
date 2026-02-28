# md_line_exact の汎用化: ブロック内行単位マッピング

## 概要

`resolve_md_line_range` (tile.rs) が計算する `md_line_exact` を、コードブロック専用
から汎用に拡張する。これにより `y` (精密ヤンク) がリスト等のブロック内でも
個別の行を返すようになる。

**変更箇所**: `tile.rs` の `resolve_md_line_range` 関数の `else` ブランチのみ。
コードブロック用の既存パスは一切変更しない。

---

## 動機

### 問題

```markdown
- item1
- item2
```

このリストに対して、`y` も `Y` も同じ結果（リスト全体の 2 行）を返す。

- `y` (exact yank): `md_line_exact = None`（コードブロックでないため）
  → `yank_lines()` にフォールバック → `md_line_range = (1, 2)` → 2 行ヤンク
- `Y` (block yank): 直接 `yank_lines()` → 同結果

ユーザーがサイドバーの行番号を見て `1y` と入力しても、`- item1` だけでなく
`- item2` も一緒に返ってしまい、直感的でない。

### 根本原因

`convert.rs` の `BlockMapping` はリスト全体を 1 つのブロックとして記録する
（`block_depth == 0` のときのみ `block_starts` に push）。
`resolve_md_line_range` は `BlockMapping.md_byte_range` 全体を行範囲に変換するため、
リスト内の個別アイテムを区別できない。

---

## 既存アルゴリズム: コードブロックの精密行マッピング

`resolve_md_line_range` (tile.rs:206-235) の既存ロジック:

```
1. BlockMapping を特定（Span → Typst バイトオフセット → 二分探索）
2. md_byte_range から start_line, end_line を算出
3. コードブロック判定: md_block_text.starts_with("```")
4. Typst テキスト内の Span 位置までの改行数を数える (newlines_before)
5. exact_line = start_line + 1 + newlines_before
   - +1 はフェンス行（```rust）をスキップするため
   - end_line - 1 にクランプ（閉じフェンス行を除外）
```

**なぜこれが機能するか:**

コードブロックでは Markdown と Typst の行構造が 1:1 で対応する。
`fill_blank_lines()` が空行にスペースを挿入して TextItem の生成を保証するが、
改行数は維持される。したがって、Typst テキスト内の改行カウントが
そのまま Markdown ソース内の行オフセットに対応する。

---

## 汎用化の検証

### 実験: 診断テストによる検証

integration test で `- item1\n- item2\n- item3\n` のフルパイプラインを実行し、
各 TextItem の Span からの改行カウントを確認した。

```
TextItem "item1": content_off=2,  typst_local=2,  newlines_before=0, estimated_md_line=1 ✓
TextItem "item2": content_off=10, typst_local=10, newlines_before=1, estimated_md_line=2 ✓
TextItem "item3": content_off=18, typst_local=18, newlines_before=2, estimated_md_line=3 ✓
```

コードブロックとの違いは `+1` オフセットが不要な点のみ
（リストにはフェンス行がないため `start_line + newlines_before` で正しい）。

### 各ブロック型の改行対応

`convert.rs` の出力を分析し、Markdown と Typst の改行数が 1:1 で対応するかを調査した。

| ブロック型 | MD→Typst 改行保存 | 理由 |
|---|---|---|
| 段落 | 条件付きYES | `SoftBreak` → `\n` だが、先行ブロックがある場合 Typst セパレータ `\n` が追加されて不一致になる |
| 見出し | 条件付きYES | 単一行, `##` → `==`。先行ブロックがある場合は段落と同様にセパレータで不一致 |
| 非順序リスト | YES | `- item\n` → `- item\n` そのまま |
| 順序リスト | YES | `1. item\n` → `+ item\n` |
| コードブロック | YES | 既存の `md_line_exact` で対応済み |
| 水平線 | YES (自明) | 単一行 → 単一行 |
| **テーブル** | **NO** | MD: `\| A \| B \|\n\|---\|---\|\n\| 1 \| 2 \|` (3行) → Typst: `#table(columns: 2,\n[A],\n[B],\n[1],\n[2],\n)` (6行) |
| **ネスト引用** | **NO** | `#quote(block: true)[` ごとに `\n\n` セパレータ挿入 |
| **単純引用** | 要検証 | `> ` 除去のみだが、`#quote(block: true)[...]` ラッパーで改行数がずれうる |

### テーブルが不可能な理由（詳細）

Markdown のテーブル:
```markdown
| A | B |
|---|---|
| 1 | 2 |
```

`convert.rs` が生成する Typst:
```typst
#table(columns: 2,
  [A],
  [B],
  [1],
  [2],
)
```

- セパレータ行 (`|---|---|`) は pulldown-cmark が消費し、出力に現れない
- 各セルが独立した行になり、元の行構造と対応しない
- 改行数: MD = 2, Typst = 5（完全不一致）

### ネスト引用が不可能な理由（詳細）

```markdown
> > inner
```
（MD: 1 行）

Typst 出力:
```typst
#quote(block: true)[
#quote(block: true)[inner]
]
```
（Typst: 3 行）

ネストレベルごとに `\n\n` + `#quote(block: true)[` が挿入され、
MD の行数と Typst の行数が乖離する。

---

## 設計: 安全メカニズム

### 改行数一致チェック

ブロック型ごとにホワイトリスト/ブラックリストを管理する代わりに、
**実際のテキストの改行数を比較する** ことで安全性を保証する。

```
md_newlines   = md_block_text 内の '\n' の数
typst_newlines = typst_block_text 内の '\n' の数

if md_newlines == typst_newlines:
    → 改行カウントで精密行を計算（安全）
else:
    → None を返す（現行動作、ブロック全体ヤンク）
```

**この方式の利点:**

1. **新しいブロック型への自動対応**: 将来 `convert.rs` に新しいブロック型が追加されても、
   改行数が保存されていれば自動的に精密ヤンクが有効になる
2. **偽陰性のみ**: 改行数が一致しない場合のフォールバックは現行動作（ブロック全体）であり、
   ユーザー体験が悪化しない
3. **偽陽性のリスク**: 改行数がたまたま一致するが行対応が崩れているケース。
   理論的にはありうるが、`convert.rs` の変換は決定的であり、
   改行数が一致していて行対応が崩れるケースは実際には発見されていない

### コードブロックの特殊処理を維持する理由

コードブロック（`` ``` `` で開始）は汎用パスでは正しく処理できない:

1. **フェンス行オフセット**: 開始フェンス行（`` ```rust ``）はコンテンツではないため
   `+1` が必要。汎用パスの `start_line + newlines_before` では 1 行ずれる
2. **終了フェンス除外**: `end_line - 1` にクランプして閉じフェンスを除外する必要がある
3. **空行処理**: `fill_blank_lines()` がコードブロック専用に適用される

したがって、`` md_block_text.starts_with("```") `` の分岐は維持し、
汎用パスは `else` ブランチにのみ適用する。

---

## VisualLine.md_line_exact の意味の拡張

### 変更前

```
md_line_exact: Option<usize>
  Some(line) — コードブロック内の精密な MD 行番号（1-based）
  None       — コードブロック以外（段落、見出し、リスト、テーブル等すべて）
```

### 変更後

```
md_line_exact: Option<usize>
  Some(line) — この visual line に対応する精密な MD 行番号（1-based）
               コードブロック: フェンス行を除いたコンテンツ行
               その他: Typst/MD 改行数が一致するブロック内の行
  None       — 精密行の計算が安全でないブロック（テーブル、ネスト引用等）
               → yank_exact() は yank_lines() にフォールバック
```

`yank_exact()` の挙動は変更不要: `md_line_exact` が `Some` なら 1 行返却、
`None` なら `yank_lines()` フォールバック。

---

## 影響範囲

### 変更されるファイル

- `src/tile.rs` — `resolve_md_line_range` の `else` ブランチ拡張 + docコメント 3 箇所

### 変更されないファイル

- `src/convert.rs` — BlockMapping の粒度は変更しない（リスト全体 = 1 ブロックのまま）
- `src/viewer/mode_normal.rs` — `y` / `Y` のディスパッチロジックは不変
- `src/viewer/input.rs` — Action enum は不変
- `src/viewer/mod.rs` — Effect の適用は不変

### ユーザー影響

| 操作 | 変更前 | 変更後 |
|---|---|---|
| リストアイテムで `Ny` | ブロック全体 | 個別アイテム 1 行 |
| リストアイテムで `NY` | ブロック全体 | ブロック全体（不変） |
| コードブロック行で `Ny` | 1 行 | 1 行（不変） |
| テーブルセルで `Ny` | ブロック全体 | ブロック全体（不変） |
| 段落で `Ny` | ブロック全体 | ブロック全体（不変、Typst セパレータにより改行数不一致） |
| 先頭ブロック（見出し等）で `Ny` | ブロック全体 | 1 行（先行ブロックがないためセパレータなし） |

**注意**: `convert.rs` は先行ブロックがある場合、Typst 出力の BlockMapping 範囲内に
`\n` セパレータを含める。このため、2 番目以降のブロック（段落・見出し等）では改行数が
不一致になり、汎用パスは自動的にフォールバックする。リストは 1 つの BlockMapping 内に
複数アイテムを含むため、セパレータの影響を受けず精密ヤンクが有効になる。
