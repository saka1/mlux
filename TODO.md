# TODO: ヤンクシステムの TUI 統合

ソースマッピングのコアインフラ（`SourceMap`, `extract_visual_lines_with_map`, `yank_lines`）は
統合テストで検証済み。以下は TUI ビューアへの統合と、ヤンク操作の完成に必要な残作業。

---

## Phase A: ソースマッピングの配管接続

viewer.rs が `markdown_to_typst()` ラッパーを使っており、`SourceMap` が捨てられている。
配管を繋ぎ替えて、`StripDocument` の `visual_lines` に `md_line_range` が入るようにする。

### A-1. viewer.rs: `markdown_to_typst_with_map()` に切り替え

```
現在:  let content_text = markdown_to_typst(&markdown);
変更:  let (content_text, source_map) = markdown_to_typst_with_map(&markdown);
```

`markdown` (原文) と `source_map` を outer loop スコープに保持する。

### A-2. `build_strip_document()` にマッピングデータを渡す

シグネチャ拡張:
```rust
fn build_strip_document(
    theme_text: &str,
    content_text: &str,
    md_source: &str,            // 追加
    source_map: &SourceMap,     // 追加
    layout: &Layout,
) -> anyhow::Result<StripDocument>
```

内部で `extract_visual_lines()` → `extract_visual_lines_with_map()` に変更し、
`SourceMappingParams` を組み立てて渡す。

### A-3. 検証

- `RUST_LOG=debug cargo run -- tests/fixtures/07_full_document.md` で
  `extract_visual_lines: N lines (N mapped, 0 unmapped)` が出ること
- 既存のスクロール動作に影響がないこと

---

## Phase B: ビジュアルモードとヤンク操作

### B-1. 選択状態の追加

イベントループに状態を追加:
```rust
struct SelectionState {
    active: bool,
    anchor_vl: usize,    // 選択開始の visual line index
    cursor_vl: usize,    // 選択カーソルの visual line index
}
```

### B-2. ピクセル Y ↔ visual line index の変換

ビューポートの現在位置から「画面中央の visual line」を特定する関数が必要。
`v` 押下時にカーソル位置（画面中央付近）の VL を anchor にする。

```rust
/// ピクセル Y 座標に最も近い visual line の index を返す
fn nearest_visual_line(visual_lines: &[VisualLine], y_px: u32) -> usize
```

### B-3. キーバインド

| キー | 動作 |
|------|------|
| `v` | ビジュアルモード ON/OFF トグル |
| `j`/`k` (ビジュアルモード中) | `cursor_vl` を移動、選択範囲を更新 |
| `d`/`u` (ビジュアルモード中) | 半画面分 `cursor_vl` をジャンプ |
| `y` (ビジュアルモード中) | 行ヤンク → クリップボードに送信 → モード解除（D-3 参照） |
| `Y` (ビジュアルモード中) | ブロックヤンク → クリップボードに送信 → モード解除 |
| `Esc` | ビジュアルモード解除 |

### B-4. `yank_lines()` の呼び出しとクリップボード送信

```rust
// ヤンク実行
let text = yank_lines(&md_source, &visual_lines, start_vl, end_vl);
// OSC 52 でクリップボードへ
let encoded = BASE64.encode(text.as_bytes());
write!(stdout(), "\x1b]52;c;{encoded}\x1b\\")?;
```

base64 エンコーダは viewer.rs に既にインポート済み（Kitty 画像送信用）。

---

## Phase C: 選択範囲のビジュアルフィードバック

### C-1. サイドバーでの選択表示

選択中の visual line をサイドバーの行番号エリアでハイライトする。
方式の候補:

- **ANSI オーバーレイ方式**: サイドバー画像の上にカーソルを移動し、
  ANSI 背景色で選択マーカーを描画（`▎` or 反転色）。
  画像の上にテキストを置くと Kitty は共存させる。
- **サイドバー再生成方式**: 選択中の行に背景色付きの行番号を含む
  サイドバー Typst を再生成・再レンダリングする。重いが確実。

初期実装は ANSI オーバーレイ方式を推奨（レンダリング不要で高速）。

### C-2. ステータスバーへのモード表示

ビジュアルモード中はステータスバーに反映:
```
 file.md | VISUAL L5-L12 | y=200/1200px  16%  [v:select y:yank Esc:cancel]
```

---

## Phase D: エッジケースと改善

### D-1. md_source の保持とリサイズ対応

outer loop でリサイズ時にドキュメントを再構築する際、`md_source` と `source_map` は
不変なのでそのまま再利用できる。`content_offset` は幅変更で変わるが
`build_strip_document()` 内で毎回再計算されるため問題ない。

### D-2. OSC 52 の制限への対処

- 一部のターミナル/tmux は OSC 52 ペイロードにサイズ制限がある（tmux: デフォルト 1MB）
- 大きな選択範囲でも通常は問題ないが、超巨大ドキュメントでは考慮が必要
- 代替: 一時ファイルに書き出して `xclip` / `pbcopy` を呼ぶフォールバック

### D-3. `y`（行ヤンク）と `Y`（ブロックヤンク）の二段構え

`Y` はブロック全体、`y` はコードブロック内なら 1 行だけヤンクする。

#### 現在のインフラで何が使えるか

`md_line_range` はブロック全体の行範囲を持つので `Y` はそのまま動く。
`y` には Span のブロック内相対オフセットから「何行目か」を導出する追加ロジックが必要。

Span 解決で得られる `content_offset` にはブロック内の位置情報が含まれている:

```
コードブロックの content_text:
  byte 0:   ```rust\n
  byte 8:   fn main() {}\n     ← VL の Span → content_offset = 8
  byte 22:  ```\n
```

`content_offset - block.typst_byte_range.start` = ブロック内バイトオフセット。
そこまでの改行数 = ブロック内行インデックス。コードブロック内では
Typst ソース ≈ MD ソース（エスケープなし）なので行番号がそのまま対応する。

#### ブロック種別ごとの `y` の挙動

| ブロック種別 | `y` の出力 | `Y` の出力 |
|---|---|---|
| コードブロック | 現在の VL に対応するコード 1 行 | フェンス含むブロック全体 |
| 見出し | 見出し行（= ブロック全体） | 同左 |
| 段落 | 段落全体（折り返しは MD 行に対応しない） | 同左 |
| リスト・テーブル・引用 | ブロック全体 | 同左 |

#### データ構造の変更

`VisualLine` にフィールドを追加:

```rust
pub struct VisualLine {
    pub y_pt: f64,
    pub y_px: u32,
    pub md_line_range: Option<(usize, usize)>,  // ブロック全体（Y 用）
    pub md_line_exact: Option<usize>,            // 精密な 1 行（y 用）
}
```

`md_line_exact` の導出（`resolve_md_line_range` 内で追加）:

1. `content_offset`（すでに計算済み）
2. `relative = content_offset - block.typst_byte_range.start`
3. `line_in_block = content_text[block.start..block.start + relative]` の改行数
4. `md_line_exact = byte_offset_to_line(md_source, block.md_byte_range.start) + line_in_block`
5. `md_line_exact` が `md_line_range` の範囲外なら `None`（安全弁）

`md_line_range.start == md_line_range.end` のブロック（見出し等）は
`md_line_exact = md_line_range.start` で自明。

#### `yank_lines` の対応

```rust
// Y: ブロックヤンク（既存の yank_lines そのまま）
let text = yank_lines(md, &vlines, start_vl, end_vl);

// y: 行ヤンク（新規関数 or パラメータ追加）
let text = yank_lines_exact(md, &vlines, start_vl, end_vl);
// → md_line_exact の union を取って該当行のみ切り出す
```

#### テストケース

```
入力: "# H\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n"
VL[2] (コードブロック 1 行目 "fn main() {") を y ヤンク
  → "fn main() {"                       ← md_line_exact
VL[2] を Y ヤンク
  → "```rust\nfn main() {\n    println!(\"hello\");\n}\n```"  ← md_line_range
```

### D-4. visual line の md_line_range が None のケース

テーマ由来のテキスト（ページ番号等）は `md_line_range = None` になる。
ヤンク範囲に含まれても `yank_lines()` がスキップするので動作は正しいが、
ビジュアルモードでの表示（選択不可マーク等）は検討の余地がある。

---

## 実装順序の推奨

```
A-1 → A-2 → A-3 → B-2 → B-1 → B-3 → B-4 → C-2 → C-1 → D-*
 配管接続        検証     VL変換  状態  キー  ヤンク  ステータス  サイドバー  改善
```

Phase A は既存テスト + viewer 動作確認で検証可能。
Phase B-4 まで完了すれば最小限のヤンク機能が使える状態になる。
