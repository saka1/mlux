# セキュリティ設計

mlux パイプライン全体のセキュリティ上の考察。
信頼できない Markdown を安全に処理するための脅威モデル、現状の防御層、
OS レベルのサンドボックスの費用対効果をまとめる。

## 脅威モデル

mlux は信頼できないソースの Markdown を開く場面がある（README、PR 本文、
ダウンロードしたドキュメントなど）。悪意ある Markdown ファイルが試みうる攻撃:

1. **パストラバーサルによる任意ファイル読み取り** (`![](../../.ssh/id_rsa)`)
2. **シンボリックリンク経由のディレクトリ脱出**
3. **SSRF** — リモート URL 指定 (`![](https://evil.com/track.png)`)
4. **DoS** — 巨大画像ファイルによるメモリ枯渇
5. **Typst コンパイラのバグを突いた任意コード実行** → ファイルシステム・ネットワークへのアクセス
6. **Typst コード注入** — markup.rs のエスケープ漏れ経由で任意の Typst スクリプトを実行

### 脅威 6 の詳細: Typst コード注入

Markdown → Typst 変換時に `escape_typst()` が `#`, `*`, `_`, `` ` ``, `[`, `]` 等の
特殊文字をエスケープする。テキスト・テーブルセル・コードブロック・インラインコードは安全。

**残存リスク**: リンク URL が未エスケープのまま `#link("{url}")` に埋め込まれる
（`markup.rs:334`）。`"` を含む URL で文字列を脱出し、任意の Typst コードを注入可能。
ただし pulldown-cmark の URL パースが `"` の扱いを制約するため、実際の攻撃成立には
パーサーの挙動次第。

注入が成功した場合、Typst スクリプトで利用可能な危険な機能:

| 機能 | World による制約 | 被害 |
|------|-----------------|------|
| `#read(path)` | `World::file()` → main.typ、テーマデータ、事前読み込み画像のみ | ドキュメント自身の内容が読める程度。任意ファイル読み取り不可 |
| `#eval(code)` | World 制約下で実行 | 上記と同じ範囲内 |
| `#import`/`#include` | `World::source()` → main.typ のみ | 外部ファイル読み込み不可 |
| `#plugin(wasm)` | `World::file()` 経由で WASM ロード | 事前読み込み画像しかないため実質機能しない |
| `sys.version` | 情報のみ | Typst バージョン漏洩（軽微） |

World の仮想ファイルシステム設計により、コード注入の被害は極めて限定的。
攻撃者が得られるのはドキュメント自身のソース（テーマ + 変換済みコンテンツ）と
事前読み込み画像のみ。

## 信頼境界

基本原則: **汚染された入力を処理するコードは全て隔離側に置く。**
render パイプライン全体（pulldown-cmark パースから PNG 出力まで）が隔離対象。

```
信頼する側 (supervisor)          │  信頼しない側 (renderer)
                                │
  Markdown バイト列読み込み       │  pulldown-cmark パース
  フォントスキャン (FontCache)    │  画像パス抽出・読み込み (Landlock scope 内)
  設定読み込み                    │  markup.rs (Typst マークアップ生成)
  ターミナル I/O                  │  typst eval / compile / render
  ファイル監視                    │  WASM プラグイン実行 (該当時)
                                │
  RPC でメタデータ・PNG 受信 ←───  タイル分割・PNG レンダリング
```

pulldown-cmark は小さく監査しやすいが、汚染入力を直接パースするため
隔離側に置く。パーサー自体のバグも防御対象とする。

## 現状の防御層

Phase 1 実装済みの現時点での防御状態。

### markup.rs のエスケープ

`escape_typst()` が Typst 特殊文字（`#`, `*`, `_`, `` ` ``, `<`, `>`, `@`,
`$`, `\\`, `/`, `~`, `(`, `)`, `[`, `]`）をバックスラッシュでエスケープ。

- テキスト・テーブルセル: `escape_typst()` 適用済み — **安全**
- コードブロック: 適応的フェンス長（コンテンツ中の最長バッククォート連続 + 1）— **安全**
- インラインコード: Typst の backtick raw モード内で実行されない — **安全**
- **リンク URL: 未エスケープ** — 残存リスクあり（上記「脅威 6」参照）

### World 仮想ファイルシステム

`MluxWorld` の `World::file()` / `World::source()` は以下のみを公開:

- `main.typ`（テーマ + 変換済みコンテンツを結合した単一ファイル）
- テーマデータファイル（ビルド時にハードコードされた tmTheme 等）
- 事前読み込み済み画像バイト列（`LoadedImages` HashMap）

それ以外の FileId に対しては `FileError::NotFound` を返す。
Typst コンパイラは `World` トレイト経由でしかファイル I/O できないため、
**任意のファイルシステムアクセスは構造的に不可能**。

### 画像パス検証 (image.rs)

- Markdown ファイルの親ディレクトリを root とし、相対パスのみ許可
- `canonicalize()` で実パスを取得し、root 配下であることを検証
- 絶対パス拒否、リモート URL（http, https, data）拒否
- サイズ上限 50MB

### Typst 標準ライブラリ

`Library::default()` で全機能が有効。`#read()`, `#eval()`, `#plugin()` 等の
危険な機能も利用可能だが、World の仮想 FS 制約により実質的な被害は限定的。
Typst 0.14 の `LibraryBuilder` は `with_inputs` / `with_features` のみ提供しており、
個別関数の無効化 API は存在しない。将来 Typst 側で API が追加されれば検討可能。

## アーキテクチャの選択肢

### Phase 1: 事前読み込み方式・単一プロセス（OS 非依存）【実装済み】

Typst コンパイル前に、すべての画像データを読み込み・検証する。
`World::file()` は `HashMap<VirtualPath, Bytes>` から返すだけで、
ディスクには一切触らない。

パス検証の手順:
- Markdown ファイルの親ディレクトリを root とする
- 画像パスを root からの相対パスとして解決
- `canonicalize()` で実パスを取得し、root 配下であることを検証
- 絶対パスとリモート URL は拒否
- サイズ上限（50MB）を設ける

制約: `canonicalize()` と読み込みの間の TOCTOU（軽微、信頼側で連続実行するため）。
typst の unsafe コードにバグがあれば任意の syscall が可能になりうる。

### Phase 1.5: render モード単独 Landlock 【実装済み】

Phase 2 の子プロセス分離を待たず、render モードのみに Landlock を適用する。
単一プロセス内で、全ファイル読み込み完了後に `restrict_self()` を呼ぶ。

```
main() → 設定読み込み → フォント読み込み → MD 読み込み → 画像読み込み
       → ★ Landlock 適用（read: git root, write: 出力ディレクトリのみ）
       → Typst コンパイル → PNG レンダリング → 出力書き込み
```

- `src/sandbox.rs` で実装。`--no-sandbox` フラグで無効化可能
- render モードは全ファイルを事前読み込みするため、Landlock 適用後にファイルアクセス不要
- viewer モードには適用不可（Landlock は不可逆 → ファイル監視等が動かなくなる）
- Linux 5.13+ で有効、非対応環境では graceful degradation
- Phase 2 で子プロセス側の Landlock に自然に移行できる

### Phase 2: fork + Landlock サンドボックス（Linux）【実装済み】

`fork()` で子プロセスを生成し、子プロセス内で render パイプライン全体を
Landlock サンドボックス下で実行する。現在 render モードで有効。

```
親プロセス:
  Markdown バイト列読み込み
  FontCache::new() (フォントスキャン)
  設定読み込み

  fork() ──→ 子プロセス:
  │            Landlock enforce_read_only_sandbox()
  │            FontCache::new()  ← 子で再作成
  │            ─────────────────────────
  │            pulldown-cmark パース      ← 汚染入力の処理開始
  │            画像パス抽出・読み込み      ← Landlock read scope 内
  │            markup.rs
  │            Typst compile
  │            タイル分割
  │            Response::Meta 送信
  │            リクエストループ (RenderTile / Shutdown)
  │
  ├── RPC: DocumentMeta 受信 (タイル数, visual lines, 幅/高さ)
  ├── RPC: RenderTile(i) → TilePngs (content + sidebar PNG)
  │
  render モード: 全タイル順次要求 → PNG ファイル書き込み
```

**実装モジュール:**

- `src/process.rs` — 汎用 fork + typed IPC 基盤
  - `TypedWriter<T>` / `TypedReader<T>`: length-prefixed bincode over pipes
  - `ChildProcess`: SIGKILL on drop による確実な子プロセス回収
  - `fork_with_channels()`: 双方向 typed channel 付き fork
- `src/fork_render.rs` — レンダラー固有の RPC プロトコル
  - `Request` enum: `RenderTile(usize)`, `Shutdown`
  - `Response` enum: `Meta(DocumentMeta)`, `Tile(TilePngs)`, `Error(String)`
  - `spawn_renderer()`: fork → sandbox 適用 → コンパイル → メタデータ送信 → リクエストループ
- `src/tile.rs` の `DocumentMeta` — RPC 境界を越えてシリアライズ可能なメタデータ
  （タイル数、画像寸法、visual lines 等）
- `src/tile.rs` の `TilePngs` — content + sidebar の PNG バイト列ペア

**render モードの流れ:**

`cmd_render_fork()` が `spawn_renderer()` で子プロセスを起動し、
`DocumentMeta` を受信後、全タイルを順次 `RenderTile(i)` で要求して
PNG ファイルに書き出す。

**viewer モードの流れ:**

viewer モードでも同じ fork+Landlock 分離を適用。outer loop の各 iteration で
`fork_renderer()` → 子プロセスがビルド → `DocumentMeta` を IPC 受信 →
inner loop 中は `RenderTile` IPC でオンデマンド描画。リサイズ・リロード・
ナビゲーション時は `ChildProcess` を drop（SIGKILL）して再 fork する。
`TileRenderer` enum が Direct（`--no-sandbox`）と Forked（IPC）を抽象化し、
prefetch worker は差異を意識しない。

**設計判断:**

- **`_exit()` vs `exit()`**: 子プロセスは `libc::_exit(0)` を使用。
  `std::process::exit()` は atexit ハンドラ（テストハーネスのスレッド join 等）で
  デッドロックするため不可。POSIX の fork 後推奨パターン。
- **COW の実態**: `spawn_renderer()` はパラメータを clone/to_string で owned 化して
  クロージャに move する。FontCache は子で再作成。COW ページ共有の実質的恩恵は
  限定的だが、別バイナリ/サブコマンド不要というアーキテクチャ上のメリットが大きい。
- **panic 隔離**: 子プロセスの `render_tile_pair()` は `catch_unwind` で囲み、
  panic しても `Response::Error` として親に返す。子プロセスのクラッシュが
  親を巻き込まない。

**Landlock ルール（子プロセス）:**

| 条件 | 読み取り許可 | 書き込み許可 |
|------|-------------|-------------|
| render モード | git root or 入力親ディレクトリ | なし（`enforce_read_only_sandbox`） |
| viewer モード（将来） | git root or 入力親ディレクトリ | なし（PNG は RPC で親に返す） |

render モードの子プロセスは読み取り専用サンドボックスで動作する。
PNG ファイル書き込みは親プロセスが RPC で受け取ったデータを書き出す。

**Linux 以外の環境:**

fork + Landlock は Linux 固有。他 OS ではフォールバック:
- 現状の単一プロセス実行（Phase 1 の World 仮想 FS による防御のみ）
- 将来的に: OpenBSD `pledge("stdio rpath", NULL)`,
  macOS/Windows は実用的な非特権サンドボックス API がないため見送り

## unsafe コードの分析

`src/process.rs` に含まれる unsafe ブロックの一覧と削減可能性:

| unsafe | 箇所 | 削減可能？ | 備考 |
|--------|------|-----------|------|
| `fork()` | L130 | 不可 | fork は本質的に unsafe（マルチスレッド環境で危険）。呼び出し前にスレッドが存在しないことを呼び出し側が保証する |
| `libc::_exit(0)` | L143 | 不可 | libc FFI。fork 後の子プロセスでは atexit ハンドラ回避のため必須 |
| `File::from_raw_fd()` | L136, L137, L150, L151 | **可** | `File::from(OwnedFd)` で safe に変換可能。`nix::pipe()` は `OwnedFd` を返すため `into_raw_fd()` → `from_raw_fd()` の往復が不要 |
| `File::from_raw_fd()` (テスト) | L174, L176, L198, L200 | **可** | 同上 |

`fork` クレート等の外部ライブラリは不採用。パイプ抽象化を提供しないため
`process.rs` を自前実装する必要は同じであり、`nix` の薄いラッパーにすぎない。

## Landlock / seccomp の費用対効果分析

### 効果が高い脅威

| 脅威 | Landlock 効果 | 理由 |
|------|-------------|------|
| コンパイラ unsafe バグ → 任意実行 | **高** | カーネルレベルで fs/net アクセス拒否。任意コード実行されてもデータ持ち出し不可 |
| 依存クレートの脆弱性 | **高** | 同上 |
| コード注入 → #read() | **中** | World で既に制約済みだが多層防御として有効 |

### 効果がない / 限定的な脅威

| 脅威 | Landlock 効果 | 理由 |
|------|-------------|------|
| DoS（CPU・メモリ枯渇） | **なし** | Landlock は CPU/メモリを制御しない。cgroup やタイムアウトが必要 |
| 注入→PNG への情報埋め込み | **なし** | 出力経路（PNG）は塞げない。漏洩先がレンダリング結果自体のため |

### 重要な洞察

**Landlock の主な正当化理由は「Typst コンパイラのメモリ安全性バグ」という
低確率・高被害の脅威に対する保険である。**

コード注入（脅威 6）に対しては World の仮想ファイルシステムがアプリケーション層の
主要防御として既に機能しており、Landlock は多層防御の追加層にすぎない。
逆に言えば、World の制約を無視できるレベルの脆弱性（メモリ破壊による任意コード実行）
に対してのみ、OS レベルのサンドボックスが本質的な防御となる。

## Typst 0.14.2 ソース確認結果

- `typst-eval`, `typst-layout`, `typst-render`, `typst-syntax`,
  `typst-realize`, `typst-html`, `typst-svg` には直接的なファイルシステム
  アクセス（`std::fs`, `File::open` 等）が**一切ない**。
  すべてのファイル I/O は `World::file()` / `World::source()` 経由。
- `typst-library` の `foundations/content/vtable.rs` に `unsafe` コードあり
  （内部 vtable ディスパッチ）。メモリ安全性のバグがあれば任意コード実行の可能性。
- `#plugin()` は `World::file()` で WASM をロードし `wasmi` で実行する。
  WASM サンドボックス自体も追加の攻撃面。
- Typst スクリプトの `#read()` は `World::file()` を呼ぶ。
  markup.rs のエスケープ漏れで Typst コード注入が起きると、
  World が公開する任意のファイルが読まれうる。

## 推奨方針

### 実装済み

- **Phase 1**: 事前読み込み + World 仮想 FS。OS 非依存の構造的防御
- **Phase 1.5**: render モード単独 Landlock（`src/sandbox.rs`）。
  render モードのみ、Typst コンパイル前にファイルアクセスを制限
- **Phase 2**: fork + Landlock（`src/process.rs`, `src/fork_render.rs`）。
  render モードで子プロセス分離 + 読み取り専用 Landlock サンドボックス
- **リンク URL エスケープ**: `escape_typst_string_literal()` で修正済み

### 次のステップ

- `File::from(OwnedFd)` による unsafe 削減（`process.rs` の `from_raw_fd()` 4 箇所 + テスト 4 箇所）
- viewer モードへの fork 分離適用（ファイル変更時の再 fork 実装が必要）

## 脅威 3 拡張: `--allow-remote-images` 有効時のリスク

`--allow-remote-images` フラグにより、Markdown 内の `http://` / `https://` URL の
画像を ureq でフェッチして表示できる。デフォルトは無効。

| 脅威 | 深刻度 | 緩和策 |
|------|--------|--------|
| トラッキングピクセル（`![](https://evil.invalid/track?id=xyz)`） | 中 | デフォルト無効。フラグ明示指定が必要 |
| SSRF — 内部ネットワークへのリクエスト（`![](http://192.168.1.1/admin)`） | 中 | v1では制限せず。ユーザーがフラグを明示的に指定する＝責任を受容 |
| 大容量レスポンスによるメモリ枯渇 | 低 | 既存の MAX_IMAGE_SIZE (50MB) 制限 + Content-Length事前チェック |
| 悪意あるレスポンス（非画像データ） | 低 | Typst の画像デコーダがパースを拒否。フォーマット検証は Typst 側に委譲 |
| DNS リバインディング | 低 | v1では対策不要。ureq は標準的な DNS 解決を使用 |

**設計判断:**
- デフォルト無効 + CLI フラグ（config.toml には入れない）= intentional friction
- フラグ名 `--allow-remote-images` は意図的に長い（誤使用防止）
- 内部IP制限（localhost, RFC 1918）は v1 では見送り。「自分の Markdown を自分で開くツール」という前提で、フラグ指定＝ユーザーの明示的判断
- タイムアウト: 10秒（ureq グローバルタイムアウト）
