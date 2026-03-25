# Fork子プロセスのログ転送設計

## 課題

`log` クレートのグローバルロガーはプロセス単位。`fork()` 後、子プロセスは親の
`RingLog` (と内部の `LogBuffer`) の COW コピーを持つが、子が書き込んだログエントリは
子のヒープに閉じ、親の `:log` ビューアには届かない。

Fork 2 はパイプライン全体（画像ロード、mermaid レンダリング、Typst コンパイル、
タイル分割）を実行するため、デバッグに有用なログが大量に失われている。

## 方針

子の `LogBuffer` に溜まったエントリを IPC レスポンスに同梱して親に転送する。
即時性は不要。レスポンス送信のタイミングでバッチ的に届けばよい。

## 設計

### 1. LogBuffer に drain を追加

```rust
// src/log.rs

impl LogBuffer {
    /// バッファ内の全エントリを取り出してクリアする。
    pub fn drain(&self) -> Vec<LogEntry> {
        let mut inner = self.inner.lock().unwrap();
        inner.entries.drain(..).collect()
    }
}
```

グローバル状態（`OnceLock` 等）は追加しない。代わりに、fork 前に `LogBuffer` の
クローンを子クロージャにキャプチャさせる（パラメータ渡し）。

fork 後、キャプチャした `LogBuffer` と `RingLog` 内部の `LogBuffer` は同じ `Arc` の
コピーであり、子プロセスのヒープ上の同一データを指す。よって `drain()` で
`log::info!()` 等が書き込んだエントリを回収できる。

### 2. IPC 用のシリアライズ可能なログエントリ

`LogEntry` は `log::Level` と `SystemTime` を含む。IPC で送るためにワイヤー型を定義する。

```rust
// src/log.rs (または renderer.rs 内 private)

#[derive(Serialize, Deserialize)]
pub struct WireLogEntry {
    /// ミリ秒 (UNIX epoch からの経過)
    pub timestamp_ms: u64,
    /// log::Level を u8 に変換 (1=Error, 2=Warn, 3=Info, 4=Debug, 5=Trace)
    pub level: u8,
    pub target: String,
    pub message: String,
}
```

`LogEntry → WireLogEntry` と `WireLogEntry → LogEntry` の変換を実装する。

### 3. IPC レスポンスをラップ

`Response` enum 自体は変更せず、ワイヤー上のメッセージをラッパーで包む。

```rust
// src/renderer.rs

#[derive(Serialize, Deserialize)]
struct ChildMessage {
    response: Response,
    logs: Vec<WireLogEntry>,
}
```

型パラメータの変更:
- `fork_with_channels::<Request, Response, _>` → `fork_with_channels::<Request, ChildMessage, _>`

子側の送信（`log_buf` は fork 前にキャプチャした `LogBuffer` クローン）:
```rust
fn send_with_logs(
    tx: &mut TypedWriter<ChildMessage>,
    response: Response,
    log_buf: &LogBuffer,
) -> Result<()> {
    let logs = log_buf.drain().into_iter().map(WireLogEntry::from).collect();
    tx.send(&ChildMessage { response, logs })
}
```

親側の受信:
```rust
fn recv_and_ingest(rx: &mut TypedReader<ChildMessage>, parent_buf: &LogBuffer) -> Result<Response> {
    let msg = rx.recv()?;
    for entry in msg.logs {
        parent_buf.push(LogEntry::from(entry));
    }
    Ok(msg.response)
}
```

### 4. fork_compute (Fork 1) にも適用

`fork_compute` は単一の結果を返す。同様にラップする。

```rust
#[derive(Serialize, Deserialize)]
struct ComputeResult<T> {
    value: T,
    logs: Vec<WireLogEntry>,
}
```

`fork_compute` のシグネチャに `LogBuffer` パラメータを追加。
子クロージャ実行後、結果とログを `ComputeResult` にまとめて送信。
親側で受信時に `LogBuffer` に push。

### 5. TileRenderer の変更

`TileRenderer` が `LogBuffer` への参照を保持し、受信時に自動的にログを取り込む。

```rust
pub struct TileRenderer {
    tx: process::TypedWriter<Request>,
    rx: process::TypedReader<ChildMessage>,
    log_buffer: LogBuffer,
}
```

`wait_for_meta`, `recv`, `try_recv` の内部で `recv_and_ingest` を使う。
外部 API は変更なし。

### 6. ログファイル (`--log`) との関係

子プロセスの `RingLog` のコピーは、親と同じ fd を持つログファイルハンドルも持つ。
子のログは従来通りファイルにも書かれる（fd が有効な限り）。
今回の変更でファイルログの挙動は変わらない。

## 変更箇所まとめ

| ファイル | 変更内容 |
|----------|----------|
| `src/log.rs` | `drain()`, `WireLogEntry` + 変換 |
| `src/renderer.rs` | `ChildMessage` ラッパー, `build_renderer` に `LogBuffer` パラメータ追加, `TileRenderer` に `LogBuffer` 追加, 送受信ロジック |
| `src/fork_sandbox/mod.rs` | `fork_compute` に `LogBuffer` パラメータ追加, `ComputeResult<T>` ラッパー適用 |
| `src/viewer/mod.rs` | `build_renderer` 呼び出し時に `log_buffer` を渡す |
| `src/main.rs` | `render` サブコマンドからの `build_renderer` 呼び出しに `log_buffer` を渡す |

## 考慮事項

- **リングバッファの容量**: 子側のバッファはデフォルト 1024 エントリ。drain で毎回クリアされるので溢れにくいが、コンパイルが大量にログを出す場合は注意。
- **エラー時のログ回収**: 子が `Response::Error` を送る際にもログを同梱するため、ビルドエラーのデバッグ情報が親に届く。
- **子がクラッシュした場合**: パイプが切れるため未送信のログは失われる。これは許容する（即時性不要の判断と整合）。
- **fork_compute (Fork 1) のログ**: prescan は軽量だがサンドボックス失敗時の warn などが回収可能になる。
