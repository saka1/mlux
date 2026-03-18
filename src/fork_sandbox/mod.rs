//! Fork-based sandboxed renderer.
//!
//! Spawns a child process that compiles the document and renders tiles on demand.
//! The child applies Landlock (read-only) before compilation, isolating the
//! render pipeline from the rest of the system.
//!
//! Internal submodules (`process`, `sandbox`) are implementation details.

mod process;
mod sandbox;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::highlight::{HighlightRect, HighlightSpec};
use crate::image::LoadedImages;
use crate::pipeline::{BuildParams, compile_and_tile};
use crate::tile::DocumentMeta;
use crate::tile_cache::TilePngs;

pub use process::ChildProcess;

/// Fork a sandboxed child that computes a value and returns it.
///
/// The child applies Landlock, runs `f()`, sends the result via IPC, and exits.
/// Panics in `f` are caught and converted to an error on the parent side
/// (child exits, pipe closes, parent recv fails).
pub fn fork_compute<T, F>(sandbox_read_base: Option<&Path>, no_sandbox: bool, f: F) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> T,
{
    let sandbox_base: Option<PathBuf> = sandbox_read_base.map(|p| p.to_path_buf());
    let (_, mut rx, mut child) =
        process::fork_with_channels::<(), T, _>(move |_req_rx, mut resp_tx| {
            if !no_sandbox && let Err(e) = sandbox::enforce_sandbox(sandbox_base.as_deref()) {
                log::warn!("child: sandbox failed: {e:#}");
            }
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                Ok(result) => {
                    let _ = resp_tx.send(&result);
                }
                Err(_) => {
                    log::error!("child: fork_compute panicked");
                }
            }
        })?;
    let result = rx.recv().context("fork_compute: child failed")?;
    child.wait()?;
    Ok(result)
}

/// Extract image paths (Fork 1, sandboxed) and fetch remote images (parent).
///
/// Fork 1 runs pulldown-cmark under a sandbox with no FS access and TCP denied.
/// The parent then fetches any remote images on the trusted side.
///
/// Returns `(all_paths, remote_images)` for passing to fork_renderer/fork_dump.
pub fn prepare_images(
    markdown: &str,
    allow_remote_images: bool,
    no_sandbox: bool,
) -> Result<(Vec<String>, LoadedImages)> {
    // Fork 1: extract image paths under sandbox (no FS, no network)
    let paths = fork_compute(None, no_sandbox, {
        let md = markdown.to_string();
        move || crate::pipeline::extract_image_paths(&md)
    })?;

    // Parent: fetch remote images (trusted side)
    let remote_images = if allow_remote_images {
        let remote_urls: Vec<String> = paths
            .iter()
            .filter(|p| p.starts_with("http://") || p.starts_with("https://"))
            .cloned()
            .collect();
        if remote_urls.is_empty() {
            LoadedImages::default()
        } else {
            let (images, errors) = crate::image::load_images(&remote_urls, None, true);
            for err in &errors {
                log::warn!("{err}");
            }
            images
        }
    } else {
        LoadedImages::default()
    };

    Ok((paths, remote_images))
}

/// Request from parent to child.
#[derive(Serialize, Deserialize)]
enum Request {
    RenderTile(usize),
    FindHighlightRects { idx: usize, spec: HighlightSpec },
    Shutdown,
}

/// Response from child to parent.
///
/// The first message is always `Meta`. Subsequent messages are `Tile`, `Rects`,
/// or `Error`.
#[derive(Serialize, Deserialize)]
enum Response {
    Meta(DocumentMeta),
    Tile {
        idx: usize,
        pngs: TilePngs,
    },
    Rects {
        idx: usize,
        rects: Vec<HighlightRect>,
    },
    Error(String),
}

/// A response from the child process, tagged with the tile index.
#[derive(Debug)]
pub enum TileResponse {
    Tile {
        idx: usize,
        pngs: TilePngs,
    },
    Rects {
        idx: usize,
        rects: Vec<HighlightRect>,
    },
}

/// Tile renderer communicating with a forked child process via typed IPC.
pub struct TileRenderer {
    tx: process::TypedWriter<Request>,
    rx: process::TypedReader<Response>,
}

impl TileRenderer {
    /// Receive the initial metadata response from the child.
    ///
    /// Must be called exactly once as the first operation after [`fork_renderer`].
    pub fn wait_for_meta(&mut self) -> Result<DocumentMeta> {
        match self
            .rx
            .recv()
            .context("failed to receive metadata from child")?
        {
            Response::Meta(m) => Ok(m),
            Response::Error(e) => anyhow::bail!("child build error: {e}"),
            _ => anyhow::bail!("unexpected response, expected Meta"),
        }
    }

    /// Send a tile render request without waiting for the response.
    pub fn send_render_tile(&mut self, idx: usize) -> Result<()> {
        self.tx.send(&Request::RenderTile(idx))
    }

    /// Send a highlight rects request without waiting for the response.
    pub fn send_find_rects(&mut self, idx: usize, spec: &HighlightSpec) -> Result<()> {
        self.tx.send(&Request::FindHighlightRects {
            idx,
            spec: spec.clone(),
        })
    }

    /// Non-blocking receive. Returns `Ok(None)` if no data is ready.
    pub fn try_recv(&mut self) -> Result<Option<TileResponse>> {
        if !self.has_pending_data() {
            return Ok(None);
        }
        self.recv().map(Some)
    }

    /// Blocking receive. Waits for the next response from the child.
    pub fn recv(&mut self) -> Result<TileResponse> {
        match self.rx.recv()? {
            Response::Tile { idx, pngs } => Ok(TileResponse::Tile { idx, pngs }),
            Response::Rects { idx, rects } => Ok(TileResponse::Rects { idx, rects }),
            Response::Error(e) => anyhow::bail!("{e}"),
            Response::Meta(_) => anyhow::bail!("unexpected Meta response"),
        }
    }

    /// Request a tile pair (content + sidebar) from the child.
    pub fn render_tile_pair(&mut self, idx: usize) -> Result<TilePngs> {
        self.send_render_tile(idx)?;
        match self.recv()? {
            TileResponse::Tile { pngs, .. } => Ok(pngs),
            _ => anyhow::bail!("unexpected response, expected Tile"),
        }
    }

    /// Request highlight rectangles for a tile's content (no rendering).
    pub fn find_highlight_rects(
        &mut self,
        idx: usize,
        spec: &HighlightSpec,
    ) -> Result<Vec<HighlightRect>> {
        self.send_find_rects(idx, spec)?;
        match self.recv()? {
            TileResponse::Rects { rects, .. } => Ok(rects),
            _ => anyhow::bail!("unexpected response, expected Rects"),
        }
    }

    /// Check if the child has sent data (non-blocking).
    pub fn has_pending_data(&self) -> bool {
        use std::os::fd::AsRawFd;
        let fd = self.rx.as_raw_fd();
        let mut pfd = nix::libc::pollfd {
            fd,
            events: nix::libc::POLLIN,
            revents: 0,
        };
        let ret = unsafe { nix::libc::poll(&mut pfd, 1, 0) };
        ret > 0 && (pfd.revents & nix::libc::POLLIN) != 0
    }

    /// Send shutdown request to the child.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(&Request::Shutdown);
    }
}

/// Fork a sandboxed renderer child process without waiting for metadata.
///
/// The child (Fork 2):
/// 1. Applies Landlock V4 sandbox (FS read-only + TCP denied)
/// 2. Loads local images within the read scope
/// 3. Merges pre-fetched remote images from parent
/// 4. Compiles the TiledDocument
/// 5. Sends `Response::Meta` back to parent
/// 6. Enters request loop: renders tiles on demand
///
/// Returns `(renderer, child_handle)`.
/// The caller must call [`TileRenderer::wait_for_meta`] to receive the first message.
pub fn fork_renderer(
    params: &BuildParams,
    image_paths: &[String],
    remote_images: LoadedImages,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<(TileRenderer, ChildProcess)> {
    let params = params.clone();
    let sandbox_read_base = sandbox_read_base.map(|p| p.to_path_buf());
    let image_paths = image_paths.to_vec();

    let (tx, rx, child) = process::fork_with_channels::<Request, Response, _>(
        move |mut req_rx: process::TypedReader<Request>,
              mut resp_tx: process::TypedWriter<Response>| {
            // SECURITY: Fork 2 applies sandbox immediately.
            // All untrusted processing (pulldown-cmark, mermaid, Typst) runs
            // under Landlock V4 (FS read-only + TCP denied).
            // Local images are loaded from disk within the read scope.
            // Remote images were pre-fetched by the parent (trusted side)
            // and passed via closure capture (COW after fork).
            if !no_sandbox && let Err(e) = sandbox::enforce_sandbox(sandbox_read_base.as_deref()) {
                log::warn!("child: sandbox failed: {e:#}");
            }

            // Load local images (Landlock read scope allows git root)
            let (mut images, errors) =
                crate::image::load_images(&image_paths, params.base_dir.as_deref(), false);
            for err in &errors {
                log::warn!("{err}");
            }

            // Merge pre-fetched remote images from parent
            images.extend(remote_images);

            // Font cache inherited from parent via fork COW (static lifetime)
            let doc = match compile_and_tile(&params, images) {
                Ok(doc) => doc,
                Err(e) => {
                    log::error!("child: build failed: {e:#}");
                    let _ = resp_tx.send(&Response::Error(format!("{e:#}")));
                    return;
                }
            };

            // Send metadata as first response
            let meta = doc.metadata();
            if resp_tx.send(&Response::Meta(meta)).is_err() {
                return;
            }

            // Request loop
            loop {
                let req = match req_rx.recv() {
                    Ok(r) => r,
                    Err(_) => break, // Parent closed channel
                };
                match req {
                    Request::RenderTile(idx) => {
                        let resp =
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                doc.render_tile_pair(idx)
                            })) {
                                Ok(Ok(pngs)) => Response::Tile { idx, pngs },
                                Ok(Err(e)) => Response::Error(format!("render tile {idx}: {e:#}")),
                                Err(_) => Response::Error(format!("render tile {idx}: panic")),
                            };
                        if resp_tx.send(&resp).is_err() {
                            break;
                        }
                    }
                    Request::FindHighlightRects { idx, spec } => {
                        let resp =
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                doc.find_tile_highlight_rects(idx, &spec)
                            })) {
                                Ok(rects) => Response::Rects { idx, rects },
                                Err(_) => Response::Error(format!(
                                    "find highlight rects tile {idx}: panic"
                                )),
                            };
                        if resp_tx.send(&resp).is_err() {
                            break;
                        }
                    }
                    Request::Shutdown => break,
                }
            }
        },
    )?;

    Ok((TileRenderer { tx, rx }, child))
}

/// Fork a sandboxed child that dumps the compiled document to stderr and exits.
///
/// The child (Fork 2) applies the sandbox, loads local images, merges
/// pre-fetched remote images, compiles the document, and writes the
/// generated Typst source and frame tree to stderr.
pub fn fork_dump(
    params: &BuildParams,
    image_paths: &[String],
    remote_images: LoadedImages,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<ChildProcess> {
    use crate::pipeline::compile_and_dump;

    let params = params.clone();
    let sandbox_read_base = sandbox_read_base.map(|p| p.to_path_buf());
    let image_paths = image_paths.to_vec();

    let (_, _, child) = process::fork_with_channels::<(), (), _>(move |_, _| {
        if !no_sandbox && let Err(e) = sandbox::enforce_sandbox(sandbox_read_base.as_deref()) {
            log::warn!("child: sandbox failed: {e:#}");
        }

        // Load local images (Landlock read scope allows git root)
        let (mut images, errors) =
            crate::image::load_images(&image_paths, params.base_dir.as_deref(), false);
        for err in &errors {
            log::warn!("{err}");
        }

        // Merge pre-fetched remote images from parent
        images.extend(remote_images);

        // Font cache inherited from parent via fork COW (static lifetime)
        if let Err(e) = compile_and_dump(&params, images) {
            eprintln!("{e:#}");
            unsafe { nix::libc::_exit(1) }
        }
    })?;

    Ok(child)
}

/// Fork and spawn a sandboxed renderer, waiting for metadata.
///
/// Convenience wrapper around [`fork_renderer`] that also receives the initial
/// `Response::Meta` message. Used by `render` mode where no loading UI is needed.
///
/// Returns `(metadata, renderer, child_handle)`.
pub fn spawn_renderer(
    params: &BuildParams,
    image_paths: &[String],
    remote_images: LoadedImages,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<(DocumentMeta, TileRenderer, ChildProcess)> {
    let (mut renderer, child) = fork_renderer(
        params,
        image_paths,
        remote_images,
        sandbox_read_base,
        no_sandbox,
    )?;
    let meta = renderer.wait_for_meta()?;
    Ok((meta, renderer, child))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_response_serde_roundtrip() {
        let req = Request::RenderTile(42);
        let encoded = bincode::serde::encode_to_vec(&req, bincode::config::standard()).unwrap();
        let (decoded, _): (Request, _) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
        match decoded {
            Request::RenderTile(idx) => assert_eq!(idx, 42),
            _ => panic!("wrong variant"),
        }

        let req2 = Request::Shutdown;
        let encoded2 = bincode::serde::encode_to_vec(&req2, bincode::config::standard()).unwrap();
        let (decoded2, _): (Request, _) =
            bincode::serde::decode_from_slice(&encoded2, bincode::config::standard()).unwrap();
        assert!(matches!(decoded2, Request::Shutdown));

        let req3 = Request::FindHighlightRects {
            idx: 7,
            spec: HighlightSpec {
                target_ranges: vec![10..20, 30..40],
                active_ranges: vec![10..20],
            },
        };
        let encoded3 = bincode::serde::encode_to_vec(&req3, bincode::config::standard()).unwrap();
        let (decoded3, _): (Request, _) =
            bincode::serde::decode_from_slice(&encoded3, bincode::config::standard()).unwrap();
        match decoded3 {
            Request::FindHighlightRects { idx, spec } => {
                assert_eq!(idx, 7);
                assert_eq!(spec.target_ranges.len(), 2);
                assert_eq!(spec.target_ranges[0], 10..20);
                assert_eq!(spec.target_ranges[1], 30..40);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_error_serde_roundtrip() {
        let resp = Response::Error("test error".into());
        let encoded = bincode::serde::encode_to_vec(&resp, bincode::config::standard()).unwrap();
        let (decoded, _): (Response, _) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
        match decoded {
            Response::Error(msg) => assert_eq!(msg, "test error"),
            _ => panic!("wrong variant"),
        }
    }
}
