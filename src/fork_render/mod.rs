//! Fork-based sandboxed renderer.
//!
//! Spawns a child process that compiles the document and renders tiles on demand.
//! The child applies Landlock (read-only) before compilation, isolating the
//! render pipeline from the rest of the system.
//!
//! Internal submodules (`process`, `sandbox`) are implementation details.

mod process;
mod sandbox;

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::highlight::{HighlightRect, HighlightSpec};
use crate::pipeline::{BuildParams, build_tiled_document};
use crate::tile::{DocumentMeta, TilePngs};

pub use process::ChildProcess;

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
    Tile(TilePngs),
    Rects(Vec<HighlightRect>),
    Error(String),
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

    /// Request a tile pair (content + sidebar) from the child.
    pub fn render_tile_pair(&mut self, idx: usize) -> Result<TilePngs> {
        self.tx.send(&Request::RenderTile(idx))?;
        match self.rx.recv()? {
            Response::Tile(pngs) => Ok(pngs),
            Response::Error(e) => anyhow::bail!("{e}"),
            _ => anyhow::bail!("unexpected response, expected Tile"),
        }
    }

    /// Request highlight rectangles for a tile's content (no rendering).
    pub fn find_highlight_rects(
        &mut self,
        idx: usize,
        spec: &HighlightSpec,
    ) -> Result<Vec<HighlightRect>> {
        self.tx.send(&Request::FindHighlightRects {
            idx,
            spec: spec.clone(),
        })?;
        match self.rx.recv()? {
            Response::Rects(rects) => Ok(rects),
            Response::Error(e) => anyhow::bail!("{e}"),
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
/// The child:
/// 1. Applies Landlock read-only sandbox
/// 2. Builds the TiledDocument
/// 3. Sends `Response::Meta` back to parent
/// 4. Enters request loop: renders tiles on demand
///
/// Returns `(renderer, child_handle)`.
/// The caller must call [`TileRenderer::wait_for_meta`] to receive the first message.
pub fn fork_renderer(
    params: &BuildParams<'_>,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<(TileRenderer, ChildProcess)> {
    // Clone owned data for the child closure (BuildParams borrows).
    let theme_name = params.theme_name.to_string();
    let theme_text = params.theme_text.to_string();
    let data_files = params.data_files;
    let markdown = params.markdown.to_string();
    let base_dir = params.base_dir.map(|p| p.to_path_buf());
    let width_pt = params.width_pt;
    let sidebar_width_pt = params.sidebar_width_pt;
    let tile_height_pt = params.tile_height_pt;
    let ppi = params.ppi;
    let allow_remote_images = params.allow_remote_images;
    let sandbox_read_base = sandbox_read_base.map(|p| p.to_path_buf());

    let (tx, rx, child) = process::fork_with_channels::<Request, Response, _>(
        move |mut req_rx: process::TypedReader<Request>,
              mut resp_tx: process::TypedWriter<Response>| {
            // Apply sandbox in child before any compilation
            if !no_sandbox
                && let Err(e) =
                    sandbox::enforce_sandbox(sandbox_read_base.as_deref(), allow_remote_images)
            {
                log::warn!("child: sandbox failed: {e:#}");
            }

            // Font cache created in child (filesystem scan, not serializable)
            let fonts = crate::pipeline::FontCache::new();

            let doc = match build_tiled_document(&BuildParams {
                theme_name: &theme_name,
                theme_text: &theme_text,
                data_files,
                markdown: &markdown,
                base_dir: base_dir.as_deref(),
                width_pt,
                sidebar_width_pt,
                tile_height_pt,
                ppi,
                fonts: &fonts,
                allow_remote_images,
            }) {
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
                                Ok(Ok(pngs)) => Response::Tile(pngs),
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
                                Ok(rects) => Response::Rects(rects),
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
/// The child applies the sandbox, compiles the document, and writes the
/// generated Typst source and frame tree to stderr. No IPC is needed because
/// the forked child shares the parent's stderr.
pub fn fork_dump(
    params: &BuildParams<'_>,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<ChildProcess> {
    use crate::pipeline::build_and_dump;

    let theme_name = params.theme_name.to_string();
    let theme_text = params.theme_text.to_string();
    let data_files = params.data_files;
    let markdown = params.markdown.to_string();
    let base_dir = params.base_dir.map(|p| p.to_path_buf());
    let width_pt = params.width_pt;
    let sidebar_width_pt = params.sidebar_width_pt;
    let tile_height_pt = params.tile_height_pt;
    let ppi = params.ppi;
    let allow_remote_images = params.allow_remote_images;
    let sandbox_read_base = sandbox_read_base.map(|p| p.to_path_buf());

    let (_, _, child) = process::fork_with_channels::<(), (), _>(move |_, _| {
        if !no_sandbox
            && let Err(e) = sandbox::enforce_read_only_sandbox(sandbox_read_base.as_deref())
        {
            log::warn!("child: sandbox failed: {e:#}");
        }

        let fonts = crate::pipeline::FontCache::new();

        if let Err(e) = build_and_dump(&BuildParams {
            theme_name: &theme_name,
            theme_text: &theme_text,
            data_files,
            markdown: &markdown,
            base_dir: base_dir.as_deref(),
            width_pt,
            sidebar_width_pt,
            tile_height_pt,
            ppi,
            fonts: &fonts,
            allow_remote_images,
        }) {
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
    params: &BuildParams<'_>,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<(DocumentMeta, TileRenderer, ChildProcess)> {
    let (mut renderer, child) = fork_renderer(params, sandbox_read_base, no_sandbox)?;
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
                pattern: "hello".into(),
                case_insensitive: true,
            },
        };
        let encoded3 = bincode::serde::encode_to_vec(&req3, bincode::config::standard()).unwrap();
        let (decoded3, _): (Request, _) =
            bincode::serde::decode_from_slice(&encoded3, bincode::config::standard()).unwrap();
        match decoded3 {
            Request::FindHighlightRects { idx, spec } => {
                assert_eq!(idx, 7);
                assert_eq!(spec.pattern, "hello");
                assert!(spec.case_insensitive);
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
