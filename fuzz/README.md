# Fuzzing

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer) によるファズテスト。

## セットアップ

```bash
cargo install cargo-fuzz
rustup install nightly-2025-11-01
```

> **Note:** 最新の nightly では `resvg` 0.45.1 がコンパイルできない（Rgba<u8> → [u8] coercion breakage）。
> `nightly-2025-11-01` (rustc 1.93.0) で動作確認済み。

## ターゲット

| ターゲット | 対象 | 速度 |
|---|---|---|
| `fuzz_convert` | `markdown_to_typst()` + SourceMap 整合性検証 | 高速（~15K exec/s） |
| `fuzz_pipeline` | convert → `MluxWorld` → `compile_document()` | 低速（~17 exec/s、毎回フォント検索） |

## 実行

```bash
# ビルド
cargo +nightly-2025-11-01 fuzz build

# convert のみ（10秒）
cargo +nightly-2025-11-01 fuzz run fuzz_convert -- -max_total_time=10

# compile まで（30秒、入力サイズ制限付き）
cargo +nightly-2025-11-01 fuzz run fuzz_pipeline -- -max_total_time=30 -max_len=4096
```

## シードコーパス

`tests/fixtures/*.md` を `fuzz/corpus/` にコピーして初期コーパスとしている。
fuzzer が生成した入力も `fuzz/corpus/` に蓄積される（`.gitignore` 済み）。

## クラッシュが見つかったら

`fuzz/artifacts/` にクラッシュ入力が保存される。

### reproduce で詳細確認

`reproduce` バイナリで「入力 Markdown → 生成 Typst → エラー内容」を確認できる:

```bash
cargo run --manifest-path fuzz/Cargo.toml --bin reproduce -- fuzz/artifacts/fuzz_pipeline/<crash-file>
```

出力例:

```
=== Input Markdown (42 bytes) ===
...
=== Generated Typst (156 bytes) ===
...
=== Compile Error ===
typst compilation failed with 1 error(s)
error: ...
```

### fuzzer で再現

```bash
cargo +nightly-2025-11-01 fuzz run fuzz_convert fuzz/artifacts/fuzz_convert/<crash-file>
```

修正後、回帰テストとして `tests/fixtures/` に追加するとよい。
