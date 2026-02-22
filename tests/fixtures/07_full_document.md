# Rustにおけるエラーハンドリング

## はじめに

Rustのエラーハンドリングは `Result<T, E>` 型を中心に設計されている。
他の言語の例外機構と異なり、**コンパイル時にエラー処理を強制**する。

## 基本パターン

### `?` 演算子

最も一般的なパターン:

```rust
fn read_config(path: &str) -> Result<Config, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}
```

### カスタムエラー型

| クレート | 特徴 | 用途 |
|----------|------|------|
| `thiserror` | derive マクロ | ライブラリ |
| `anyhow` | 動的エラー型 | アプリケーション |
| `eyre` | カスタムレポート | CLIツール |

> **Note**: ライブラリでは `thiserror`、アプリケーションでは `anyhow` が
> 一般的な選択肢とされている。
>
> > ただし、この使い分けは絶対的なルールではない。

## エラー処理の手順

1. エラー型を定義する
2. `Result<T, E>` で返す
3. 呼び出し元で `?` 演算子を使う

避けるべきパターン:

- `unwrap()` の多用
- エラーの握りつぶし
- ~~`panic!` による強制終了~~（テスト以外では非推奨）

## まとめ

エラーハンドリングの設計は——プロジェクトの性質に応じて——柔軟に選択すべきである。
詳細は [The Rust Programming Language](https://doc.rust-lang.org/book/) を参照。

---

*最終更新: 2025年2月*
