# 画像表示機能: セキュリティ設計

画像表示（`![alt](path)`）を mlux に追加する際のセキュリティ上の考察。
実装範囲は未定だが、脅威モデルとアーキテクチャの選択肢をここに残す。

## 脅威モデル

mlux は信頼できないソースの Markdown を開く場面がある（README、PR 本文、
ダウンロードしたドキュメントなど）。悪意ある Markdown ファイルが試みうる攻撃:

1. **パストラバーサルによる任意ファイル読み取り** (`![](../../.ssh/id_rsa)`)
2. **シンボリックリンク経由のディレクトリ脱出**
3. **SSRF** — リモート URL 指定 (`![](https://evil.com/track.png)`)
4. **DoS** — 巨大画像ファイルによるメモリ枯渇
5. **Typst コンパイラのバグを突いた任意コード実行** → ファイルシステム・ネットワークへのアクセス

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

## アーキテクチャの選択肢

### Phase 1: 事前読み込み方式・単一プロセス（OS 非依存）

Typst コンパイル前に、すべての画像データを読み込み・検証する。
`World::file()` は `HashMap<VirtualPath, Bytes>` から返すだけで、
ディスクには一切触らない。

パス検証の手順:
- Markdown ファイルの親ディレクトリを root とする
- 画像パスを root からの相対パスとして解決
- `canonicalize()` で実パスを取得し、root 配下であることを検証
- 絶対パスとリモート URL は拒否
- サイズ上限（例: 50MB）を設ける

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

Phase 2（子プロセス分離）は、自動化された環境で信頼できない入力を処理する
場合に追加する。Phase 3（Landlock/seccomp）は高セキュリティ環境向けの多層防御。

設計上の要点: Phase 2 を後から追加できるよう、レンダリングパイプラインは
ファイルパスではなく事前読み込み済みバイト列を受け取る API にしておくこと。
