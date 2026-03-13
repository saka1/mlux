# 視覚行抽出: フレームツリーからの行位置検出

## 概要

`PagedDocument` のフレームツリーから視覚行（= ユーザーが認識する1行）を抽出する際に
発見された2つの問題と対策を記録する。

1. **ベースラインオフセット問題** — 同一行内の TextItem が微妙に異なる Y 座標を持つ
2. **コードブロック空行問題** — 空行に TextItem が生成されない

## 背景

`extract_visual_lines()` は全 `TextItem` の絶対 Y 座標を収集し、
近接する Y 座標を dedup して「1つの Y = 1視覚行」とする。

---

## 問題1: 混在フォントサイズのベースラインオフセット

### 症状

dedup の tolerance が不適切だと、同一行が複数エントリに分裂し、
サイドバー行番号が重なって表示される。

### 原因

Typst のフレームツリーで `TextItem` の `(Point, FrameItem)` が示す Y 座標は
**ベースライン位置** である。同一視覚行に異なるフォントサイズのテキストが混在すると、
各テキストアイテムのベースライン Y 座標が微妙にずれる。

### オフセットパターン（実測値）

入力: `tests/fixtures/08_long_prose.md`, テーマ: `catppuccin.typ`

| パターン | オフセット | 原因 |
|---|---|---|
| 12pt 本文 vs 10pt インラインコード | **+0.59pt** | フォントメトリクス差（IPAGothic 12pt vs DejaVu Sans Mono 10pt） |
| インラインコードの box inset | **+2.0pt** | テーマの `raw.where(block: false): box(inset: (y: 2pt))` |
| リストマーカー "•" vs 本文テキスト | **+2.59pt** | 上記2つの複合 |

### 実例: `- **不変参照** (\`&T\`)` の行

```
Y=532.77pt  "•"         (12pt IPAGothic, リストマーカー)
Y=533.36pt  "&T"        (10pt DejaVu Sans Mono, インラインコード) ← +0.59pt
Y=535.36pt  "不変参照"   (12pt IPAGothic, 本文テキスト)           ← +2.00pt (inset)
```

1つの視覚行が **3つの異なる Y 座標** を持つ。

### テーブルセルでも発生

```
Y=1028.20pt  "インターフェー"  (12pt, セルテキスト)
Y=1028.78pt  "trait"           (10pt, インラインコード)  ← +0.59pt
```

## 行間ギャップとの比較

| 行間の種類 | 典型的なギャップ |
|---|---|
| 本文行送り (12pt + 1em leading) | **21.0pt** |
| 見出し (H3) → 本文 | **15.0pt**（最小） |
| 段落間 | **23.4pt** |
| 見出し (H2) → 本文 | **17.6pt** |
| セクション間 (H2 見出し) | **36.6pt** |

行内オフセットの最大値 **~2.6pt** と行間最小ギャップ **15.0pt** には十分な差がある。

## 対策: dedup tolerance = 5.0pt

```
行内最大オフセット: ~2.6pt (0.59 + 2.0)
行間最小ギャップ:   15.0pt (H3 → 本文)
安全マージン:       5.0pt < 15.0 / 2 = 7.5pt
```

tolerance を 5.0pt に設定することで:
- ✅ 行内のフォントサイズ差による Y ズレを確実にマージ
- ✅ 最小行間ギャップ (15pt) を分離可能
- ✅ 安全マージン: tolerance (5pt) は最小ギャップの 1/3

### 修正前後の比較

`08_long_prose.md` (width=580pt, ppi=144):
- 修正前 (tolerance=0.5pt): **95** visual lines（11行が偽のエントリ）
- 修正後 (tolerance=5.0pt): **84** visual lines（全ギャップ ≥ 15pt）

## 将来の考慮事項

### tolerance の限界

現在の tolerance は catppuccin テーマの設定に依存する:
- `raw.where(block: false)` の `inset: (y: 2pt)` が変わればオフセットも変わる
- 異なるフォントの組み合わせではメトリクス差が変わる

テーマのパラメータを大幅に変更する場合は、再検証が必要。

### より堅牢な代替アプローチ

tolerance ベースの dedup ではなく、以下の方法も検討可能:

1. **Group ベースの行検出**: トップレベル Group の Y 座標をベースに行を特定
   - Group は折り返し行ごとに分かれるため、TextItem より粒度が粗い
   - ただし直接 Text として出現するケース（Group に包まれない短い段落）がある

2. **フォントサイズフィルタリング**: 本文サイズ (12pt) の TextItem のみを収集
   - インラインコードやマーカーのオフセット問題を回避
   - ただし見出し (24pt, 20pt, 16pt) やコードブロック (10pt) の行を見落とす

現状の tolerance=5.0pt は実用的に十分であり、上記の代替案より実装が単純。

---

## 問題2: コードブロック内の空行が検出されない

### 症状

コードブロック内の空行に行番号が振られない。

### 原因: Typst は空行に TextItem を生成しない

Typst はコードブロック（raw ブロック）内の空行を **Y方向のスペース** として処理し、
TextItem を生成しない。`extract_visual_lines()` は TextItem の Y 座標のみを収集するため、
空行の位置を検出できない。

### 実測データ

入力: `tests/fixtures/08_long_prose.md` 第2コードブロック

```
use std::num::ParseIntError;   Y=1373.5pt
                                        ← 35.2pt gap (= 2x normal)  ← 空行！
#[derive(Debug)]                Y=1408.7pt
```

| 行間の種類 | ギャップ | 正常行間との比率 |
|---|---|---|
| コードブロック通常行間 (10pt DejaVu Sans Mono) | **17.6pt** | 1.0x |
| コードブロック空行あり | **35.2pt** | 2.0x（ちょうど2倍） |
| 本文段落間 | **23.4pt** | 1.11x（正常行間 21pt 基準） |

### 検討した代替アプローチ: ギャップベースの推定

`extract_visual_lines()` 内で Y ギャップから空行を推定する方法:

- コードブロック行をフォントサイズ (~10pt) で識別
- ギャップ >= 1.5 × 正常行間 なら空行ありと判定
- 合成 VisualLine を挿入

**不採用の理由**: フレームツリーの後処理で推定するより、
変換レイヤで根本対処する方が単純で確実。

### 採用した対策: 変換時にスペース文字を挿入

`markup.rs` の `fill_blank_lines()` で、コードブロック内の空行に
スペース1文字を挿入する。

```
Markdown:      "line1\n\nline3"
Typst markup:  "line1\n \nline3"
                       ^ スペース1文字
```

#### なぜ機能するか

Typst の raw ブロック内でスペース1文字の行は TextItem を生成する（実測確認済み）:

```
TextItem  (20.0, 26.1)pt  size=8.0pt  glyphs=1  text="line1"
TextItem  (20.0, 37.4)pt  size=8.0pt  glyphs=1  text=" "    ← スペースでも TextItem が生成される
TextItem  (20.0, 48.6)pt  size=8.0pt  glyphs=1  text="line3"
```

- `extract_visual_lines()` はそのまま Y 座標を拾える（変更不要）
- `generate_sidebar_typst()` も変更不要（連番で行番号を振るだけ）

#### ヤンク機能への影響

空行にスペース1文字が残るが、実用上は無害:
- ターミナルペースト時に視覚的差異なし
- 多くのエディタは保存時に trailing whitespace を除去
- 必要なら、ヤンク実装時に `text.trim() == ""` のフィルタを1行追加で対処可能

---

## 問題3: 未認識言語のコードブロックが1視覚行に潰れる

### 症状

言語タグなし（` ``` `）や Typst が認識しない言語（`console` 等）のコードブロックが、
行数に関係なく1つの視覚行として扱われる。サイドバー行番号が1行しか表示されない。

### 原因: フレームツリー構造の違い

認識言語と未認識言語でフレームツリーの構造が異なる:

| 言語 | フレームツリー | `should_recurse()` |
|---|---|---|
| 認識言語 (`rust`, `toml` 等) | RawLine Tags + per-line **Groups** (syntax highlighting) | `has_line_structure()` で検出 ✅ |
| 未認識言語 (`console`, なし) | RawLine Tags + bare **Text** items (Groups なし) | 検出不可 ❌ |

`should_recurse()` は `has_line_structure()` (child Groups の垂直配置) と
`has_dominant_child_group()` (50%超の child Group) のみで判定していたため、
Groups を持たないコードブロックは再帰されず、1視覚行に潰れていた。

### RawLine Tag による検出

Typst の `RawLine` 要素は `#[elem(name = "line", Tagged)]` として定義されている。
`Tagged` trait により、言語認識の有無にかかわらず全行に `Tag::Start`/`Tag::End` ペアが
フレームツリーに挿入される。

#### Typst の raw ブロック処理フロー

```
RawElem → synthesize() → highlight() or non_highlighted_result()
  → per-line RawLine → RAW_RULE show function
  → LinebreakElem で行分割
```

`highlight()` は認識言語に対してシンタックスハイライト付きの子要素を生成し、
`non_highlighted_result()` は未認識言語に対してプレーンテキストの子要素を生成する。
いずれの場合も `RawLine` は `Tagged` なので Tag ペアが出力される。

### 対策: `has_raw_line_tags()`

`should_recurse()` の第3条件として `has_raw_line_tags()` を追加:

```rust
fn has_raw_line_tags(frame: &Frame) -> bool {
    frame.items().any(|(_, item)| {
        if let FrameItem::Tag(Tag::Start(content, _)) = item {
            content.elem().name() == "line"
        } else {
            false
        }
    })
}
```

RawLine Tag が検出された場合、フレームを再帰し、bare Text items を
`flush_pending_texts` の Y-tolerance グルーピングで個別の視覚行に分割する。

### 影響範囲

- コードブロック（raw 要素）のみに影響
- リスト、段落、数式等は child Groups を持つため既存のパスで処理される
- 認識言語のコードブロックは `has_line_structure()` が先に true を返すため変化なし

### 備考: `#metadata()` はフレームツリーに現れない

カスタムアンカーの代替として `#metadata()` の使用を検討したが、
`Locatable` のみで `Tagged` ではないため、フレームツリーに Tag として現れない。
RawLine の既存 Tag を利用する方が正しいアプローチ。
