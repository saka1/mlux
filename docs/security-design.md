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
6. **Typst コード注入** — convert.rs のエスケープ漏れ経由で任意の Typst スクリプトを実行

### 脅威 6 の詳細: Typst コード注入

Markdown → Typst 変換時に `escape_typst()` が `#`, `*`, `_`, `` ` ``, `[`, `]` 等の
特殊文字をエスケープする。テキスト・テーブルセル・コードブロック・インラインコードは安全。

**残存リスク**: リンク URL が未エスケープのまま `#link("{url}")` に埋め込まれる
（`convert.rs:334`）。`"` を含む URL で文字列を脱出し、任意の Typst コードを注入可能。
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

アーキテクチャ上の核心は、どこに信頼境界を引くかにある。

```
信頼する側 (supervisor)          │  信頼しない側 (renderer)
                                │
  Markdown ファイル読み込み       │  convert.rs (Typst マークアップ生成)
  pulldown-cmark パース          │  typst eval / compile
  画像パスの収集・検証・読み込み   │  typst render
  バイト列を pipe で送信 ────────→  WASM プラグイン実行 (該当時)
                                │
  PNG 受信 ←────────────────────  PNG 出力
```

pulldown-cmark は小さく監査しやすいため信頼側に置く。
Typst マークアップ生成以降（typst-eval, typst-layout, typst-library の
unsafe vtable コード, wasmi WASM ランタイム）は攻撃面が広く、
理想的には隔離して実行する。

## 現状の防御層

Phase 1 実装済みの現時点での防御状態。

### convert.rs のエスケープ

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

### Phase 2: 子プロセス分離（OS 非依存）

Typst コンパイルを子プロセスで実行する。全データ（マークアップ + 画像バイト列）を
stdin パイプで送り、PNG を stdout で受け取る。

```rust
let child = Command::new(current_exe()?)
    .arg("--sandbox-worker")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .spawn()?;
```

子プロセスはファイルパスもオープン済み fd（stdin/stdout/stderr 以外）も
持たない。typst に任意コード実行バグがあっても、持ち出すデータがない。
ただし子プロセスは新たに syscall で fd を開くことは可能（→ Phase 3 で対処）。

起動時に継承 fd を閉じる:
```rust
close_fds_above(2);
```

### Phase 3: OS レベルのサンドボックス（Linux）

Linux 5.13+ では、子プロセス内で Landlock + seccomp-bpf を適用し、
ファイルシステム・ネットワークアクセスをカーネルレベルで不可能にする。

- **Landlock**: 許可パスを一切追加しないルールセットを作成 → 全ファイルアクセス拒否
- **seccomp-bpf**: `read`, `write`, `mmap`, `munmap`, `brk`, `close`,
  `exit_group` のみホワイトリスト許可。それ以外の syscall（open, openat,
  socket, connect, execve）はプロセスを即座に kill。

多層防御として機能し、子プロセス内で任意コード実行されても
ファイルを開くことも、ネットワークで送信することもできない。

他 OS のサポート状況:
- OpenBSD: `pledge("stdio", NULL)` で同等の制限が可能
- macOS: `sandbox-exec`（Seatbelt）は deprecated で公開 API の代替がない
- Windows: Restricted Token + Job Object で制限可能だが API が複雑

#### Phase 3 代替案: render モード単独 Landlock

子プロセス分離（Phase 2）を待たず、render モードのみに Landlock を適用する案。

```
main() → 設定読み込み → フォント読み込み → MD 読み込み → 画像読み込み
       → ★ Landlock 適用（出力先ディレクトリのみ書き込み許可、他は全拒否）
       → Typst コンパイル → PNG レンダリング → 出力書き込み
```

- render モードは全ファイルを Typst コンパイル前に読み込むため、
  Landlock 適用後にファイルアクセスが不要
- viewer モードには適用不可（ファイル監視、設定リロード、URL オープンが
  Landlock 適用後に必要。Landlock はプロセス存続期間中解除不可）
- Linux 5.13+ で有効、非対応環境では graceful degradation（スキップ）
- Phase 2 + Phase 3 の完全な子プロセス分離より実装が大幅に単純

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
  convert.rs のエスケープ漏れで Typst コード注入が起きると、
  World が公開する任意のファイルが読まれうる。

## 推奨方針

Phase 1（事前読み込み + HashMap）で初期実装としては十分。
OS 固有のコードなしで、Typst コンパイラが構造的に任意ファイルにアクセスできない
状態を作れる。

**最優先**: リンク URL のエスケープ修正（低コスト・即効性）。
convert.rs のエスケープ漏れは、World の制約により被害が限定的とはいえ、
防御層の一つが欠損している状態であり、早期に修正すべき。

**render モード Landlock**（Phase 3 代替案）は Phase 2 の子プロセス分離を待たずに
適用可能であり、コスト対効果が最も高い段階的アプローチ。

Phase 2（子プロセス分離）は、自動化された環境で信頼できない入力を処理する
場合に追加する。Phase 3（子プロセス + Landlock/seccomp）は高セキュリティ環境向けの
多層防御。

設計上の要点: Phase 2 を後から追加できるよう、レンダリングパイプラインは
ファイルパスではなく事前読み込み済みバイト列を受け取る API にしておくこと。
