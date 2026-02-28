# mlux Viewer Specification

ターミナルビューア (`mlux <file>`) の動作仕様。

## Modes

4 つのモードを持つ。起動時は Normal。

| Mode | 入口 | 用途 |
|------|------|------|
| Normal | 起動時 / 他モードから戻る | タイル表示 + スクロール |
| Search | `/` | Markdown ソースの正規表現 grep |
| Command | `:` | コマンド実行 |
| URL Picker | `O` / `:open` / `No` | URL 選択・オープン |

---

## Normal Mode

### Navigation

| Key | Action | 備考 |
|-----|--------|------|
| `j` / `↓` | 下スクロール | `scroll_step` セル分 |
| `k` / `↑` | 上スクロール | |
| `d` | 半ページ下 | `image_rows / 2` セル (min 1) |
| `u` | 半ページ上 | |
| `g` | 先頭へ | prefix なしの場合 |
| `G` | 末尾へ | |
| `Ng` / `NG` | N 行目へジャンプ | visual line index (1-based) |

すべてのナビゲーションキーに数値 prefix 対応 (`5j`, `10d` など)。

### Quit

| Key | Action |
|-----|--------|
| `q` | 終了 |
| `Ctrl-C` | 終了 |
| `Esc` | 数値 prefix キャンセル |

### Yank (コピー)

| Key | Action |
|-----|--------|
| `Ny` | N 行目の Markdown ソースを yank |
| `NY` | N 行目のブロックを yank |
| `y` / `Y` | prefix なし → ヒント flash 表示 |

OSC 52 でクリップボードに送信。`md_line_range` のない行は失敗 (flash 通知)。

### URL

| Key | Action |
|-----|--------|
| `No` | N 行目の URL を開く (単一なら直接、複数なら picker) |
| `o` | prefix なし → ヒント flash 表示 |
| `O` | 全 URL の picker を開く |

### Search Navigation

| Key | Action |
|-----|--------|
| `/` | Search mode へ |
| `n` | 次のマッチへジャンプ (wrap) |
| `N` | 前のマッチへジャンプ (wrap) |

`n` / `N` は直前の `/` 検索結果を使う。検索履歴がない場合は何もしない。

### Numeric Prefix

- 数字キーで蓄積 (最大 999,999)
- コマンドキーで消費、`Esc` でキャンセル
- ステータスバーに蓄積中の数字を表示

---

## Search Mode

`/` で入る。Markdown ソース全行を正規表現で grep し、結果をリスト表示する。

### 画面レイアウト

```
Row 0:     /query_           ← プロンプト
Row 1..N:  検索結果リスト    ← 選択行はハイライト
Last row:  ステータスバー    ← マッチ数 + キーヒント
```

### 検索仕様

| 項目 | 仕様 |
|------|------|
| エンジン | `regex` クレート (`RegexBuilder`) |
| Smartcase | クエリが全小文字 → case-insensitive、大文字を含む → case-sensitive |
| 無効パターン | 0 件マッチ + ステータスバーに `invalid pattern` (赤背景) |
| 空クエリ | 0 件 (pattern_valid = true) |
| マッチ単位 | 行単位 (1 行に 1 マッチ、最初のヒット位置をハイライト) |

入力ごとにリアルタイム再検索。

### キーバインド

| Key | Action |
|-----|--------|
| (文字) | クエリに追加 |
| `Backspace` | 末尾削除 |
| `j` / `↓` | 次の結果を選択 |
| `k` / `↑` | 前の結果を選択 |
| `Enter` | 選択行へジャンプ → Normal mode |
| `Esc` / `Ctrl-C` | キャンセル → Normal mode |

`Enter` 確定時、検索結果は `LastSearch` に保存され `n`/`N` で再利用される。

---

## Command Mode

`:` で入る。

### コマンド一覧

| Command | 短縮 | Action |
|---------|------|--------|
| `:quit` | `:q` | 終了 |
| `:reload` | `:rel` | config.toml 再読み込み (CLI override 維持) |
| `:open` | — | 全 URL の picker を開く |

未知のコマンドは flash メッセージ表示して Normal に戻る。

### キーバインド

| Key | Action |
|-----|--------|
| (文字) | 入力に追加 |
| `Backspace` | 削除。空入力で Backspace → キャンセル |
| `Enter` | 実行 |
| `Esc` / `Ctrl-C` | キャンセル |

---

## URL Picker Mode

`O` または `:open` で入る。`No` で特定行の URL が複数ある場合も入る。

### URL 抽出

- `[text](url)` 形式のリンク
- 本文中の bare HTTP(S) URL
- 同一 `md_line_range` の重複は除外

### キーバインド

| Key | Action |
|-----|--------|
| `j` / `↓` | 次の URL を選択 |
| `k` / `↑` | 前の URL を選択 |
| `Enter` | 選択 URL をブラウザで開く |
| `Esc` / `Ctrl-C` | キャンセル |

---

## Config

`~/.config/mlux/config.toml` (`$XDG_CONFIG_HOME` 対応)。

```toml
theme = "catppuccin"   # テーマ名
width = 660.0          # ページ幅 (pt)
ppi = 144.0            # 描画解像度

[viewer]
scroll_step = 3        # j/k 1 回あたりのセル数
frame_budget_ms = 32   # フレームバジェット (ms)
tile_height = 500.0    # タイル高さ (pt, 最小値)
sidebar_cols = 6       # サイドバー幅 (列数)
evict_distance = 4     # キャッシュ破棄距離 (タイル数)
watch_interval_ms = 200  # ファイル監視間隔 (ms)
```

上記はすべてデフォルト値。すべてのフィールドは省略可能。

### 優先順位

1. ハードコードデフォルト
2. config.toml
3. CLI オプション (`--theme`, `--width`, `--ppi`, `--tile-height`)

CLI override は `:reload` でも維持される。

---

## CLI

```
mlux [OPTIONS] <file>                      # ビューアモード
mlux [OPTIONS] render <file> -o <out.png>  # PNG レンダリング
```

### グローバルオプション

| Flag | 説明 |
|------|------|
| `--theme <name>` | テーマ名 |
| `--no-watch` | ファイル監視を無効化 |
| `--log <path>` | ログ出力先 |

### `render` サブコマンド

| Flag | 説明 | デフォルト |
|------|------|-----------|
| `-o <path>` | 出力 PNG パス | `output.png` |
| `--width <pt>` | ページ幅 | 660.0 |
| `--ppi <n>` | 解像度 | 144.0 |
| `--tile-height <pt>` | タイル高さ | 500.0 |
| `--dump` | Frame tree をダンプ | — |

---

## File Watching

- デフォルト有効 (`--no-watch` で無効化)
- 親ディレクトリを監視 (Linux inotify が atomic save で watch を失うため)
- 変更検出 → Markdown 再読み込み → ドキュメント再構築
- `y_offset` は再構築後も維持

---

## Status Bar

最下行に常時表示。

```
{filename} | L{current_line}/{total_lines} | [{prefix}] | [{flash}]
```

- **prefix**: 数値蓄積中に表示
- **flash**: 操作フィードバック (1 キー入力で消える)
  - 例: `Yanked L42 (2 lines)`, `match 3/5`, `Config reloaded`
