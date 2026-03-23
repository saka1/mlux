//! Domain-specific orchestration layer.
//!
//! Provides high-level APIs that combine image preparation (Fork 1) with
//! sandboxed rendering (Fork 2). Callers supply `BuildParams` and get back
//! a renderer or dump result — all fork/sandbox/IPC details are hidden.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::fork_sandbox::process;
use crate::fork_sandbox::sandbox;
use crate::highlight::{HighlightRect, HighlightSpec};
use crate::image::LoadedImages;
use crate::pipeline::{BuildParams, compile_and_dump, compile_and_tile};
use crate::tile::DocumentMeta;
use crate::tile_cache::TilePngs;

pub use crate::fork_sandbox::process::ChildProcess;

// ---------------------------------------------------------------------------
// IPC protocol types (private)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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
    /// Must be called exactly once as the first operation after [`build_renderer`].
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

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Extract image paths (Fork 1, sandboxed) and fetch remote images (parent).
///
/// Fork 1 runs pulldown-cmark under a sandbox with no FS access and TCP denied.
/// The parent then fetches any remote images on the trusted side.
fn prepare_remote_images(
    params: &BuildParams,
    no_sandbox: bool,
) -> Result<(crate::pipeline::Prescan, LoadedImages)> {
    use crate::fork_sandbox::fork_compute;

    // Fork 1: prescan under sandbox (no FS, no network)
    let prescan_result = fork_compute(None, &[], no_sandbox, {
        let md = params.markdown.clone();
        move || crate::pipeline::prescan(&md)
    })?;
    let paths = &prescan_result.image_paths;

    // Parent: fetch remote images (trusted side)
    let remote_images = if params.allow_remote_images {
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

    Ok((prescan_result, remote_images))
}

/// Derive the sandbox read base path from BuildParams.
fn sandbox_read_base(params: &BuildParams) -> Option<&Path> {
    params.base_dir.as_deref()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a sandboxed renderer: prepare images (Fork 1) + fork renderer (Fork 2).
///
/// Returns `(renderer, child_handle)` without waiting for metadata.
/// The caller must call [`TileRenderer::wait_for_meta`] to receive the first message.
pub fn build_renderer(
    params: &BuildParams,
    no_sandbox: bool,
) -> Result<(TileRenderer, ChildProcess)> {
    let (prescan, remote_images) = prepare_remote_images(params, no_sandbox)?;
    let read_base = sandbox_read_base(params);
    let font_dirs = params.fonts.font_dirs();

    let params = params.clone();
    let read_base = read_base.map(|p| p.to_path_buf());

    let (tx, rx, child) = process::fork_with_channels::<Request, Response, _>(
        move |mut req_rx: process::TypedReader<Request>,
              mut resp_tx: process::TypedWriter<Response>| {
            // SECURITY: Fork 2 applies sandbox immediately.
            if !no_sandbox
                && let Err(e) = sandbox::enforce_sandbox(read_base.as_deref(), &font_dirs)
            {
                log::warn!("child: sandbox failed: {e:#}");
            }

            // Load local images (Landlock read scope allows git root)
            let (mut images, errors) =
                crate::image::load_images(&prescan.image_paths, params.base_dir.as_deref(), false);
            for err in &errors {
                log::warn!("{err}");
            }

            // Merge pre-fetched remote images from parent
            images.extend(remote_images);

            // Font cache inherited from parent via fork COW (static lifetime)
            let doc = match compile_and_tile(&params, &prescan, images) {
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

/// Build a sandboxed renderer and wait for metadata.
///
/// Convenience wrapper around [`build_renderer`] that also receives the initial
/// `Response::Meta` message. Used by `render` mode where no loading UI is needed.
///
/// Returns `(metadata, renderer, child_handle)`.
pub fn build_renderer_blocking(
    params: &BuildParams,
    no_sandbox: bool,
) -> Result<(DocumentMeta, TileRenderer, ChildProcess)> {
    let (mut renderer, child) = build_renderer(params, no_sandbox)?;
    let meta = renderer.wait_for_meta()?;
    Ok((meta, renderer, child))
}

/// Build and dump: prepare images (Fork 1) + fork dump (Fork 2).
///
/// The child compiles the document and writes the generated Typst source
/// and frame tree to stderr, then exits.
pub fn build_dump(params: &BuildParams, no_sandbox: bool) -> Result<ChildProcess> {
    let (prescan, remote_images) = prepare_remote_images(params, no_sandbox)?;
    let read_base = sandbox_read_base(params);
    let font_dirs = params.fonts.font_dirs();

    let params = params.clone();
    let read_base = read_base.map(|p| p.to_path_buf());

    let (_, _, child) = process::fork_with_channels::<(), (), _>(move |_, _| {
        if !no_sandbox && let Err(e) = sandbox::enforce_sandbox(read_base.as_deref(), &font_dirs) {
            log::warn!("child: sandbox failed: {e:#}");
        }

        // Load local images (Landlock read scope allows git root)
        let (mut images, errors) =
            crate::image::load_images(&prescan.image_paths, params.base_dir.as_deref(), false);
        for err in &errors {
            log::warn!("{err}");
        }

        // Merge pre-fetched remote images from parent
        images.extend(remote_images);

        // Font cache inherited from parent via fork COW (static lifetime)
        if let Err(e) = compile_and_dump(&params, &prescan, images) {
            eprintln!("{e:#}");
            unsafe { nix::libc::_exit(1) }
        }
    })?;

    Ok(child)
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
