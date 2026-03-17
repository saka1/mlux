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

**変更後の順序（fetch → sandbox → compile）:**
```
fork → fetch_resources()     ← サンドボックス前にフェッチ完了
     → enforce_sandbox()     ← /etc 等の許可不要。FS read-only + TCP 全禁止
     → compile_and_tile()    ← 完全にサンドボックス下
```

サンドボックス適用前にフェッチが完了するため、ネットワーク系システムパスを
read scope に含める必要がなくなる。これはサンドボックスの強化でもある
（`/etc` への読み取りが不要 = 攻撃面が狭まる）。

### 分割

```
fetch_resources(params) -> Resources
  extract_image_paths → load_images  (FS + ネットワーク I/O)
  extract_diagrams → render_diagrams (mermaid SVG 生成)

compile_and_tile(params, resources) -> TiledDocument
  markdown_to_typst  (純粋変換)
  compile_document   (Typst コンパイル)
  split_frame        (フレーム分割)
```

`build_tiled_document()` は互換性のために残し、内部で両関数を呼ぶラッパーとする。
ただし `fork_renderer()` / `fork_dump()` の子プロセスクロージャ内では、ラッパーを使わず
`fetch_resources()` と `compile_and_tile()` を個別に呼び、その間にサンドボックス適用を挟む。

注: 現在の実装では `build_tiled_document()` と `build_and_dump()` は内部で共通の
`compile_content()` を呼んでおり、画像読み込みはこの関数内で行われる。
リファクタリングの実際のターゲットは `compile_content()` の分割である。

### 子プロセスのライフサイクル

```
fork →
  Phase 1: fetch_resources()     ← ネットワーク・FS アクセスあり
  Phase 2: enforce_sandbox()     ← Landlock V4 適用 (FS read-only + TCP 全禁止)
           FontCache::new()      ← フォントスキャン (read scope 内)
  Phase 3: compile_and_tile()    ← サンドボックス下で実行
  → Meta 送信 → リクエストループ
```

### 具体例: `--allow-remote-images` 時の動作

1. ユーザーが `![](https://example.com/photo.png)` を含む Markdown を開く
2. Phase 1: `fetch_resources()` が ureq で HTTPS フェッチ → `Resources` に PNG バイト列を格納
3. Phase 2: `enforce_sandbox()` が Landlock V4 を適用。以後 TCP connect は全て拒否
4. Phase 3: `compile_and_tile()` が `Resources` 内の画像バイト列を使って Typst コンパイル
5. ユーザーが Markdown を編集し、新しいリモート画像 URL を追加
6. viewer がリロード → 子プロセスを SIGKILL → 新しい子を fork → Phase 1 から再開

### 安全性不変条件

**Phase 1 → 2 → 3 の順序は安全性の根幹であり、変更してはならない。**

この不変条件は `fork_render/mod.rs` の子プロセスクロージャ内にコードコメントとして記載する:

```rust
// SECURITY: The ordering of these three phases is critical.
//
// Phase 1 (fetch_resources): Network and filesystem I/O happen BEFORE
//   sandboxing. Remote images (--allow-remote-images) are fetched here.
//
// Phase 2 (enforce_sandbox): Landlock V4 locks down both filesystem
//   (read-only to git root) and network (all TCP bind/connect denied).
//   This is irreversible — once applied, no new files or connections.
//
// Phase 3 (compile_and_tile): Typst compilation and rendering run
//   fully sandboxed. Even if a dependency crate or Typst itself has
//   an arbitrary code execution bug, it cannot reach the network or
//   access files outside the read scope.
//
// Do NOT reorder these phases or move I/O into Phase 3.
```

### `--allow-remote-images` なしの場合

Phase 1 にネットワークアクセスが不要だが、コードパス統一のため同じ 3 フェーズ構造を使う。
Landlock V4 のネットワーク制限は `--allow-remote-images` の有無に関わらず常に適用する。

注: `allow_remote_images` パラメータは `enforce_sandbox()` からは削除されるが、
`fetch_resources()` には引き続き必要（リモート画像のフェッチ可否を制御するため）。

### `fork_dump` の扱い

`fork_dump()` (`--dump` フラグ) も同じ fetch → sandbox → compile の順序に変更する。
現在は `enforce_read_only_sandbox()` → `build_and_dump()` の順だが、
`build_and_dump()` 内部の `load_images()` をフェッチ段階に分離する。
`build_and_dump()` は `fetch_resources()` + `compile_and_dump()` に分割するか、
既存の `fetch_resources()` を呼んだ上で dump 用のコンパイル関数を呼ぶ形にする。

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

### 注: mermaid レンダリングのセキュリティ

`fetch_resources()` 内の mermaid ダイアグラム生成 (`mermaid-rs-renderer`) は内部で
ヘッドレス Chromium を起動し JavaScript を実行する。これはサンドボックス適用前（Phase 1）
に行われるため機能上の問題はないが、Chromium プロセス自体が攻撃面となりうる。
本変更のスコープ外だが、将来的に mermaid レンダリングの隔離も検討に値する。
