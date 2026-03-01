# stdin Pipe Input: Design Decisions

## Overview

mlux supports incremental reading from stdin pipes (`curl ... | mlux`, `gh issue view | mlux`).
Data is displayed as it arrives, with the document rebuilding on each new chunk.

## Architecture: Reader Thread + mpsc Channel

```
[reader thread]                [main thread]
  loop {                         outer loop {
    stdin.read(&mut buf)           rx.try_recv() drain -> stdin_buf
    tx.send(Data(chunk))           build_tiled_document(&stdin_buf)
  }                                inner loop {
  tx.send(Eof)                       event::poll(timeout=200ms)
                                     handle keyboard...
                                     rx.try_recv() drain -> stdin_buf
                                     if new data -> Reload
                                   }
                                 }
```

### Why thread + channel, not non-blocking I/O

- Non-blocking read requires `fcntl(F_SETFL, O_NONBLOCK)`, which is Unix-specific.
- Thread + `mpsc::channel` uses only `std`, no platform-specific code.
- Effective latency is identical: both are bounded by the inner loop's poll cycle (200ms).
- Kitty Graphics Protocol targets Linux/Mac, but no reason to close off Windows.

### Zero shared state

The reader thread and main thread share no state. The channel transfers ownership
of `String` chunks. `stdin_buf` is a local `String` in the outer loop scope.
No `Arc`, no `Mutex`, no atomics.

## Keyboard Input with stdin as Pipe

crossterm's `use-dev-tty` feature reads keyboard events from `/dev/tty` on Unix,
bypassing stdin entirely. This allows keyboard input even when stdin is a pipe.
On Windows, crossterm uses the Console API for keyboard events, which is separate
from stdin. The `use-dev-tty` feature is `#[cfg(unix)]` internally and does not
affect Windows builds.

Because keyboard input never comes from stdin, `check_tty()` only verifies that
stdout is a terminal. There is no stdin terminal check — stdin may be a pipe in
any mode (file mode with unrelated pipe, or explicit stdin mode). This also means
`cat file | mlux other.md` works without issue.

## Rate Limiting

No explicit debounce logic is needed. Two natural layers provide rate limiting:

1. **Inner loop poll timeout (200ms)**: `event::poll(timeout)` + `try_recv()` drain
   coalesces rapid chunks into a single batch.
2. **Outer loop compile time (100-500ms)**: While Typst compiles, data accumulates
   in the channel. The next drain collects it all at once.

## UTF-8 Boundary Handling

`read()` may split multi-byte characters. `String::from_utf8_lossy` handles this
with replacement characters. The next drain + recompile (within 200ms) replaces
the partial data with correct content, so the artifact is transient.

## Edge Cases

- **No data yet**: Placeholder `*(waiting for input...)*` is displayed.
- **Slow trickle**: Natural coalescing via poll interval + compile time.
- **Large burst**: Reader thread fills channel; main thread drains all at once.
- **EOF**: `StdinChunk::Eof` sets `stdin_eof = true`, switching idle timeout
  to 86400s (effectively infinite), stopping the polling loop.
- **File mode**: Completely unchanged; `InputSource::File` paths are a refactor only.

## Detection Logic

stdin mode is activated when:
- CLI argument is `-` (explicit): `mlux -` or `mlux render - -o out.png`
- No input argument and stdin is not a terminal (auto-detect): `echo "# Hi" | mlux`
