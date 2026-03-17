# Security Architecture Reassessment

Previous document: [`docs/2026-03-07-design-security.md`](2026-03-07-design-security.md)

## Summary

Reassessed the fork+Landlock architecture. Conclusion: **fork structure is retained** and enhanced with a 2-stage fork design, Landlock V4 (network restrictions), and pipeline splitting.

## Why Keep Fork

1. **Sandbox irreversibility**: Landlock/seccomp cannot be relaxed after `restrict_self()`. Fork-based disposable processes allow re-sandboxing on each viewer reload.

2. **`--allow-remote-images` + viewer reload**: Remote image fetching must happen before sandboxing. Fork gives a clean lifecycle: fetch -> sandbox -> compile on each rebuild.

3. **Supply chain defense**: Restricting TCP via Landlock V4 in the child process prevents malicious crate dependencies from phoning home, even if they achieve arbitrary code execution during compilation.

## Architecture: 2-Stage Fork

```
Fork 1 (sandbox: no FS access + V4 TCP denied):
  extract_image_paths(markdown)    <- pulldown-cmark under sandbox
  -> Vec<String> returned to parent -> exit

Parent (trusted):
  Classify and fetch remote URLs (--allow-remote-images only)

Fork 2 (sandbox: FS read-only git root + V4 TCP denied):
  Load local images from disk      <- within Landlock read scope
  Merge with pre-fetched remote images from parent (COW after fork)
  compile_and_tile()               <- fully sandboxed
  -> Meta -> tile request loop
```

### Key Properties

- **pulldown-cmark always sandboxed** (Fork 1, no FS/no network)
- **Network I/O only in parent** (trusted side)
- **Fork 2 applies Landlock V4 immediately** — local image loading happens within the read scope
- **Same flow with or without `--allow-remote-images`** (remote fetch is empty when disabled)

## Change 1: Landlock V3 -> V4

- `ABI::V3` -> `ABI::V4` in `sandbox.rs`
- `AccessNet::from_all(abi)` added to `handle_access()` chain
- No `NetPort` rules = all TCP bind/connect denied
- On V3 kernels, `AccessNet::from_all()` returns empty flags -> graceful degradation

| Kernel | ABI | FS Restriction | Network Restriction |
|--------|-----|----------------|---------------------|
| 6.7+   | V4  | Yes            | Yes                 |
| 6.2-6.6| V3 fallback | Yes   | No                  |
| < 6.2  | V1-V2 fallback | Partial | No             |
| < 5.13 | None | No            | No                  |

## Change 2: Sandbox Simplification

- `enforce_sandbox(read_base, allow_network)` -> `enforce_sandbox(read_base)`
- `NETWORK_SYSTEM_PATHS` (`/etc`, `/usr/lib`, `/run`) removed — no longer needed since network I/O happens before sandboxing
- `enforce_read_only_sandbox()` removed (was identical to simplified `enforce_sandbox`)
- resolv.conf symlink resolution logic removed

## Change 3: Pipeline Split

`compile_content()` no longer loads images internally. New public functions:

- `compile_and_tile(params, images)` — core pipeline from pre-loaded images
- `compile_and_dump(params, images)` — dump variant from pre-loaded images
- `build_tiled_document(params)` — compatibility wrapper (loads images then delegates)

`fork_renderer` / `fork_dump` now receive `image_paths` and `remote_images` as arguments.

## Change 4: `fork_compute` Abstraction

Generic "fork -> sandbox -> compute -> return result" helper used by Fork 1:

```rust
pub fn fork_compute<T, F>(sandbox_read_base, no_sandbox, f) -> Result<T>
```

`prepare_images()` orchestrates Fork 1 + parent-side remote fetch:

```rust
pub fn prepare_images(markdown, allow_remote_images, no_sandbox) -> Result<(Vec<String>, LoadedImages)>
```

## Change 5: Unsafe Reduction

8 instances of `unsafe { File::from_raw_fd(fd.into_raw_fd()) }` replaced with safe `File::from(owned_fd)` in `process.rs`. Remaining unsafe: `fork()` and `libc::_exit()` (inherently unsafe).

## Future Work

- seccomp syscall filtering (Phase 2 after Landlock V4 evaluation)
- `execve`, `fork`, `clone`, `ptrace` denial in child processes
