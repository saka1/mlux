# TODO: ヤンクシステムの TUI 統合

ソースマッピングのコアインフラ（`SourceMap`, `extract_visual_lines_with_map`, `yank_lines`）は
統合テストで検証済み。Vim/less風の数字プレフィックス+コマンドUIを採用。

---

## 完了済み

### Phase A: ソースマッピングの配管接続 ✓

- `viewer.rs` を `markdown_to_typst_with_map()` に切り替え
- `build_tiled_document()` に `md_source`, `source_map` を渡す
- `extract_visual_lines_with_map()` で `SourceMappingParams` を構築
- `visual_lines` に `md_line_range` が入ることを確認

### Phase B: Vim 風数字プレフィックス UI ✓

- `InputAccumulator` 状態マシン（数字蓄積 → コマンドディスパッチ）
- カウント付きスクロール: `10j`, `10k`, `3d`, `3u`
- 行ジャンプ: `56g`, `56G`（数字なしは従来通り先頭/末尾）
- ヤンク: `56y`, `56Y` → `yank_lines()` → OSC 52 クリップボード
- ステータスバー拡張: 数字蓄積中は `:56_` 表示、ヤンク成功時はフラッシュメッセージ
- `Esc` で数字入力キャンセル

### D-1. `y` と `Y` の区別（精密ヤンク） ✓

- `VisualLine` に `md_line_exact: Option<usize>` フィールド追加
- `resolve_md_line_range` がコードブロック（`"```"` 開始）を検出し、Typst テキスト内の改行数から精密行を算出
- `yank_exact()` 関数追加: `md_line_exact` があれば1行返却、なければ `yank_lines` フォールバック
- `y` → `yank_exact()`（コードブロックで1行）、`Y` → `yank_lines()`（ブロック全体）
- 統合テスト3件追加

---

## 残作業

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

---

## アイデア・メモ

### convert.rs の fuzzing

- `cargo-fuzz` (libFuzzer) でランダムな Markdown を convert → typst compile に通す
- 目標: どんな入力でも convert.rs が valid Typst を出力することを保証する
- fuzz target: `Markdown → convert() → typst::compile()` でコンパイルエラーなら fail
- 見つかったクラッシュケースはそのまま回帰テストに追加
- pulldown-cmark 自体は十分 fuzz されているので、mlux 固有の変換ロジックが対象
- 背景: 内部で Typst 変換をしている都合上、エラーメッセージがユーザーにとって意味不明になりやすい。
  壊れた Markdown も寛容に受け付けて valid Typst を出力し、「見た目がおかしい」という形で伝えるのが理想。

### URLを開く

- markdownリンクの位置を追跡できているので、それを open する操作もキーボードベースで実現できるはず

### ターミナル E2E テスト（KGP シミュレータ）

- 背景: 検索モード→Normal復帰時に画面が戻らないバグがあった。Effect層のロジックは正しいが、
  ターミナル I/O（画像の削除・再配置・テキストクリア）の組み合わせで発生。ユニットテストでは捕まらない。
- 方針: KGP のサブセット（5コマンド: `a=t`, `a=p`, `a=d,d=i/I/A`）の状態トラッカーを作り、
  「Confirm 後に placements が非空か」等をアサートする E2E テスト。
- ステップ:
  1. `terminal.rs` の全関数を `&mut impl Write` 受けに抽象化（最重量だがログ/リプレイにも使える）
  2. 出力バイト列から APC (`\x1b_G...\x1b\\`) をパースする KGP ステートトラッカー（100-200行）
  3. 入力側は `Vec<Event>` を食わせるアダプタ
- KGP パーサ自体は小さい。本当に重いのは stdout/stdin の分離リファクタ。
