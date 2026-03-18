# セキュリティアーキテクチャ再評価

前提ドキュメント: [`docs/2026-03-07-design-security.md`](../../2026-03-07-design-security.md)

## 背景

fork + Landlock による現行アーキテクチャを再評価した。
当初の問い: 「Landlock が防御の主体なら fork 構造を廃止してコードを単純化できないか？」

## 再評価の結論

**fork 構造は維持する。** 理由:

1. **Landlock / seccomp の不可逆性 + fork の使い捨て性**: サンドボックスは `restrict_self()` 後に緩められない。fork による使い捨てプロセスにより、リロードのたびに「I/O → サンドボックス適用 → コンパイル」のライフサイクルを繰り返せる。単一プロセスでは不可能。

2. **`--allow-remote-images` + viewer リロード**: フラグを提供する以上、そのモードでも最善のセキュリティを提供すべき。子プロセスが画像フェッチ後にネットワークを封じることで、コンパイル段階での外部通信を防ぐ。fork なしの単一プロセスでは、リロード時のネットワーク再封鎖か機能制限かの二択になる。

3. **サプライチェーン防御**: Rust クレートの依存ツリーは大きく、全クレートの監査は非現実的。子プロセスで TCP 通信を Landlock V4 で禁止し、将来 seccomp で execve 等も禁止することで、悪意あるクレートによる外部通信・プログラム起動を OS レベルで防ぐ。

### 検討した代替案

| 案 | 評価 |
|----|------|
| fork 廃止 + 単一プロセス Landlock + seccomp | seccomp の不可逆性により viewer リロード時に `--allow-remote-images` と両立不可 |
| fork 廃止 + Landlock のみ | execve / socket を封じられず、サプライチェーン攻撃の主要シナリオを防げない |
| self-exec（自バイナリを `Command::new` で再起動） | IPC・プロセス管理の複雑さは fork と同等。コード単純化にならない |

## 変更 1: Landlock V3 → V4 アップグレード + ネットワーク制限

### 概要

Landlock ABI を V3 (ファイルシステムのみ) から V4 (ファイルシステム + ネットワーク) に上げる。
`AccessNet::BindTcp` / `AccessNet::ConnectTcp` を `handle_access` に追加し、
ネットワーク許可ルールを付与しないことで TCP 通信を全面禁止する。

### 対象ファイル

- `src/fork_render/sandbox.rs`

### 変更内容

実際の Ruleset 構築は `sandbox.rs` 内の `imp::enforce()` 関数で行われる。変更はこの関数内:

- `let abi = ABI::V3` → `let abi = ABI::V4`
- `Ruleset::default().handle_access(all_access)` に `.handle_access(AccessNet::from_all(abi))` を追加
- ネットワークルール（`NetPort`）は付与しない = 全 TCP bind/connect 禁止
- `Ruleset::default()` は現状通り使用する（ABI 自動交渉ではなく、FS/Net ルール構築時に明示 ABI を使う）。
  `AccessFs::from_all(ABI::V4)` は V4 で追加された FS 権限（`IOCTL_DEV` 等）も含む。
  `AccessNet::from_all(ABI::V4)` は V4 未対応カーネルでは空の `BitFlags` を返すため、
  ネットワーク制限は自動的にスキップされる。FS ルールは V3 相当にフォールバックする。

### graceful degradation

| カーネル | ABI | FS 制限 | ネットワーク制限 |
|---------|-----|---------|----------------|
| 6.7+ | V4 | 有効 | 有効 |
| 6.2 - 6.6 | V3 フォールバック | 有効 | なし |
| 5.13 - 6.1 | V1-V2 フォールバック | 部分的 | なし |
| < 5.13 | なし | なし | なし |

### `enforce_sandbox` の簡素化

パイプライン分割（変更 2）により、サンドボックス適用時点ではネットワーク I/O は完了済み。
したがって `enforce_sandbox` の `allow_network` パラメータと `NETWORK_SYSTEM_PATHS`
（`/etc`, `/usr/lib`, `/run` への読み取り許可）は**不要になり、削除する**。

変更前: `enforce_sandbox(read_base, allow_network)` — ネットワーク系パスを条件付きで追加
変更後: `enforce_sandbox(read_base)` — FS read-only + TCP 全禁止のみ

`enforce_read_only_sandbox` は `enforce_sandbox` と同義になるため統合・削除する。

## 変更 2: パイプライン分割（fetch / compile 分離）

### 現状からの変更点

**現在の順序（sandbox → fetch+compile）:**
```
fork → enforce_sandbox(allow_network=true)  ← DNS/TLS 用に /etc 等を許可
     → build_tiled_document()               ← 画像フェッチ + コンパイルが混在
```

`--allow-remote-images` 時、サンドボックス下でネットワークアクセスするために
`/etc`, `/usr/lib`, `/run` を read scope に追加していた。

**実装: 2 段 fork アーキテクチャ:**
```
Fork 1 (sandbox: FS なし + V4 TCP 拒否):
  extract_image_paths(markdown)  ← pulldown-cmark もサンドボックス下
  → Vec<String> を親に返す → exit

Parent (trusted):
  リモート URL を分類・フェッチ (--allow-remote-images 時のみ)

Fork 2 (sandbox: FS read-only git root + V4 TCP 拒否):
  load local images from disk    ← Landlock read scope 内
  merge with remote images       ← 親からクロージャ capture (COW)
  compile_and_tile()             ← 完全サンドボックス下
  → Meta 送信 → リクエストループ
```

pulldown-cmark を含む全ての信頼できないコードがサンドボックス下で実行される。
ネットワーク I/O は親プロセスのみ（trusted side）。
ネットワーク系システムパス（`/etc`, `/usr/lib`, `/run`）の read scope 追加は不要。

### 分割

```
prepare_images(markdown, allow_remote) -> (Vec<String>, LoadedImages)
  Fork 1: fork_compute(None) { extract_image_paths(markdown) }  ← サンドボックス下
  Parent: fetch remote URLs → LoadedImages

compile_and_tile(params, images) -> TiledDocument
  extract_diagrams → render_diagrams (mermaid → SVG: 純粋計算、I/O なし)
  markdown_to_typst  (純粋変換)
  compile_document   (Typst コンパイル)
  split_frame        (フレーム分割)
```

`prepare_images` は Fork 1 でパス抽出 + 親側でリモートフェッチを行う。
`render_diagrams` (mermaid-rs-renderer) は純粋な計算処理でありネットワーク・FS アクセスを
行わないため、サンドボックス後の `compile_and_tile` 側に配置する。

`build_tiled_document()` は互換性のために残し、内部で画像読み込み + `compile_and_tile` を
呼ぶラッパーとする。テストや非 fork パスから使用。

`compile_content()` は画像読み込みを外部に分離し、`LoadedImages` を引数で受け取る。

### Fork 2 のライフサイクル

```
Fork 2 →
  Phase 1: enforce_sandbox()     ← Landlock V4 適用 (FS read-only + TCP 全禁止)
  Phase 2: load_images()         ← ローカル画像読み込み (read scope 内)
           extend(remote_images) ← 親からのリモート画像マージ
           FontCache::new()      ← フォントスキャン (read scope 内)
  Phase 3: compile_and_tile()    ← サンドボックス下で実行
  → Meta 送信 → リクエストループ
```

### 具体例: `--allow-remote-images` 時の動作

1. ユーザーが `![](https://example.com/photo.png)` を含む Markdown を開く
2. Fork 1: `fork_compute` で `extract_image_paths()` を実行（サンドボックス下）→ パスリスト返却
3. 親: リモート URL を ureq で HTTPS フェッチ → `LoadedImages` に PNG バイト列を格納
4. Fork 2: `enforce_sandbox()` が Landlock V4 を適用。以後 TCP connect は全て拒否
5. Fork 2: ローカル画像読み込み + リモート画像マージ → `compile_and_tile()` で Typst コンパイル
6. ユーザーが Markdown を編集し、新しいリモート画像 URL を追加
7. viewer がリロード → Fork 2 を SIGKILL → Fork 1 から再開

### 安全性不変条件

**Phase 1 → 2 → 3 の順序は安全性の根幹であり、変更してはならない。**

この不変条件は `fork_render/mod.rs` の子プロセスクロージャ内にコードコメントとして記載する:

```rust
// SECURITY: Fork 2 applies sandbox immediately.
// All untrusted processing (pulldown-cmark, mermaid, Typst) runs
// under Landlock V4 (FS read-only + TCP denied).
// Local images are loaded from disk within the read scope.
// Remote images were pre-fetched by the parent (trusted side)
// and passed via closure capture (COW after fork).
```

### `--allow-remote-images` なしの場合

Phase 1 にネットワークアクセスが不要だが、コードパス統一のため同じ 3 フェーズ構造を使う。
Landlock V4 のネットワーク制限は `--allow-remote-images` の有無に関わらず常に適用する。

注: `allow_remote_images` パラメータは `enforce_sandbox()` からは削除されるが、
`fetch_resources()` には引き続き必要（リモート画像のフェッチ可否を制御するため）。

### `fork_dump` の扱い

`fork_dump()` (`--dump` フラグ) も同じ 2 段 fork アーキテクチャを使用。
`prepare_images()` (Fork 1 + 親側フェッチ) → `fork_dump()` (Fork 2: sandbox → load local → merge remote → `compile_and_dump()`)。

## 変更 3: unsafe 削減

### 対象ファイル

- `src/fork_render/process.rs`

### 変更内容

`nix::pipe()` が返す `OwnedFd` を `File::from(OwnedFd)` で safe に変換する。
`unsafe { File::from_raw_fd(fd.into_raw_fd()) }` の 8 箇所を置換。

```rust
// Before (unsafe)
let reader = TypedReader::new(unsafe { File::from_raw_fd(p2c_read.into_raw_fd()) });

// After (safe)
let reader = TypedReader::new(File::from(p2c_read));
```

- `fork_with_channels` 内: 4 箇所
- テスト内: 4 箇所
- `use std::os::fd::{FromRawFd, IntoRawFd}` import を削除

`process.rs` 内に残る unsafe: `fork()` と `libc::_exit()` の 2 箇所。本質的に unsafe であり削減不可。

注: モジュール全体では `fork_render/mod.rs` の `libc::poll()` (has_pending_data),
`libc::_exit(1)` (fork_dump) にも unsafe がある。これらは本変更のスコープ外。

## 変更 4: セキュリティ設計ドキュメント

本スペックに基づき `docs/2026-03-18-design-security-reassessment.md` を新規作成する。
既存の `docs/2026-03-07-design-security.md` は変更しない。
本スペックファイルは実装後に削除してよい。

## 将来課題: seccomp

Landlock V4 でネットワークを封じた上で、seccomp で syscall をさらに制限する。

### 子プロセスの seccomp フィルタ（2 フェーズ方式）

**フェーズ 1 (ビルド段階):** Landlock と併用。`execve`, `fork`, `clone`, `ptrace` を禁止。
ネットワーク syscall (`connect`, `socket`) は `--allow-remote-images` 時に必要なためこの段階では許可。

**フェーズ 2 (レンダリング段階):** 全ファイル・画像フェッチ完了後に追加フィルタ適用。
最小限の syscall のみ許可（`read`, `write`, `mmap`, `munmap`, `mprotect`, `brk`,
`clock_gettime`, `close`, `sigaltstack`, `rt_sigaction`, `exit_group` 等）。
それ以外は KILL。

注: 上記の許可リストは出発点であり、実装時に `strace` / seccomp audit モードで
実際に必要な syscall を網羅的に検証する必要がある（`futex`, `getrandom`, `writev`,
`sched_yield` 等が追加で必要になる可能性が高い）。

### 実装方針

- `seccompiler` クレート (Firecracker 由来) または直接 BPF
- 非対応環境では graceful degradation
- `prctl(PR_SET_NO_NEW_PRIVS, 1)` が前提条件

### 優先度

Landlock V4 ネットワーク制限の実装後。実装コストが高いため、Landlock で得られる防御が
十分かを運用で評価した上で判断する。

### 注: mermaid レンダリング

`mermaid-rs-renderer` (default-features = false) は純粋な Rust 計算であり、
ネットワーク・FS アクセスやヘッドレスブラウザの起動は行わない。
Phase 3（サンドボックス後）で安全に実行できる。
