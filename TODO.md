# TODO: ヤンクシステムの TUI 統合

ソースマッピングのコアインフラ（`SourceMap`, `extract_visual_lines_with_map`, `yank_lines`）は
統合テストで検証済み。Vim/less風の数字プレフィックス+コマンドUIを採用。

---

## 完了済み

### Phase A: ソースマッピングの配管接続 ✓

- `viewer.rs` を `markdown_to_typst_with_map()` に切り替え
- `build_strip_document()` に `md_source`, `source_map` を渡す
- `extract_visual_lines_with_map()` で `SourceMappingParams` を構築
- `visual_lines` に `md_line_range` が入ることを確認

### Phase B: Vim 風数字プレフィックス UI ✓

- `InputAccumulator` 状態マシン（数字蓄積 → コマンドディスパッチ）
- カウント付きスクロール: `10j`, `10k`, `3d`, `3u`
- 行ジャンプ: `56g`, `56G`（数字なしは従来通り先頭/末尾）
- ヤンク: `56y`, `56Y` → `yank_lines()` → OSC 52 クリップボード
- ステータスバー拡張: 数字蓄積中は `:56_` 表示、ヤンク成功時はフラッシュメッセージ
- `Esc` で数字入力キャンセル

---

## 残作業

### D-1. `y` と `Y` の区別（精密ヤンク）

現在 `y` = `Y` = ブロック粒度ヤンク。
`md_line_exact` を `VisualLine` に追加し、`y` だけ精密な1行ヤンクにする。

#### ブロック種別ごとの `y` の挙動（将来）

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

### D-2. OSC 52 の制限への対処

- 一部のターミナル/tmux は OSC 52 ペイロードにサイズ制限がある（tmux: デフォルト 1MB）
- 代替: 一時ファイルに書き出して `xclip` / `pbcopy` を呼ぶフォールバック

### D-3. 範囲ヤンク

必要になったら `N,My` 構文で範囲指定ヤンクを追加。
`InputAccumulator` を拡張して `RangeStart(n)`, `RangeEnd(n, m)` 状態を追加する。

### D-4. visual line の md_line_range が None のケース

テーマ由来のテキスト（ページ番号等）は `md_line_range = None` になる。
ヤンク範囲に含まれても `yank_lines()` がスキップするので動作は正しいが、
ビジュアルモードでの表示（選択不可マーク等）は検討の余地がある。
