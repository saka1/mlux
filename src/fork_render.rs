//! Fork-based sandboxed renderer.
//!
//! Spawns a child process that compiles the document and renders tiles on demand.
//! The child applies Landlock (read-only) before compilation, isolating the
//! render pipeline from the rest of the system.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::pipeline::{BuildParams, build_tiled_document};
use crate::process::{ChildProcess, TypedReader, TypedWriter, fork_with_channels};
use crate::tile::{DocumentMeta, TilePngs};

/// Request from parent to child.
#[derive(Serialize, Deserialize)]
pub enum Request {
    RenderTile(usize),
    Shutdown,
}

/// Response from child to parent.
///
/// The first message is always `Meta`. Subsequent messages are `Tile` or `Error`.
#[derive(Serialize, Deserialize)]
pub enum Response {
    Meta(DocumentMeta),
    Tile(TilePngs),
    Error(String),
}

/// Fork a sandboxed renderer child process without waiting for metadata.
///
/// The child:
/// 1. Applies Landlock read-only sandbox
/// 2. Builds the TiledDocument
/// 3. Sends `Response::Meta` back to parent
/// 4. Enters request loop: renders tiles on demand
///
/// Returns `(request_writer, response_reader, child_handle)`.
/// The caller must receive `Response::Meta` as the first message.
pub fn fork_renderer(
    params: &BuildParams<'_>,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<(TypedWriter<Request>, TypedReader<Response>, ChildProcess)> {
    // Clone owned data for the child closure (BuildParams borrows).
    let theme_text = params.theme_text.to_string();
    let data_files = params.data_files;
    let content_text = params.content_text.to_string();
    let md_source = params.md_source.to_string();
    let source_map = params.source_map.clone();
    let width_pt = params.width_pt;
    let sidebar_width_pt = params.sidebar_width_pt;
    let tile_height_pt = params.tile_height_pt;
    let ppi = params.ppi;
    let image_files = params.image_files.clone();
    let sandbox_read_base = sandbox_read_base.map(|p| p.to_path_buf());

    fork_with_channels::<Request, Response, _>(
        move |mut req_rx: TypedReader<Request>, mut resp_tx: TypedWriter<Response>| {
            // Apply sandbox in child before any compilation
            if !no_sandbox
                && let Err(e) =
                    crate::sandbox::enforce_read_only_sandbox(sandbox_read_base.as_deref())
            {
                log::warn!("child: sandbox failed: {e:#}");
            }

            // Font cache created in child (filesystem scan, not serializable)
            let fonts = crate::pipeline::FontCache::new();

            let doc = match build_tiled_document(&BuildParams {
                theme_text: &theme_text,
                data_files,
                content_text: &content_text,
                md_source: &md_source,
                source_map: &source_map,
                width_pt,
                sidebar_width_pt,
                tile_height_pt,
                ppi,
                fonts: &fonts,
                image_files,
            }) {
                Ok(doc) => doc,
                Err(e) => {
                    log::error!("child: build failed: {e:#}");
                    // Parent will get broken pipe on recv
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
                    Request::Shutdown => break,
                }
            }
        },
    )
}

/// Fork and spawn a sandboxed renderer, waiting for metadata.
///
/// Convenience wrapper around [`fork_renderer`] that also receives the initial
/// `Response::Meta` message. Used by `render` mode where no loading UI is needed.
///
/// Returns `(metadata, request_writer, response_reader, child_handle)`.
pub fn spawn_renderer(
    params: &BuildParams<'_>,
    sandbox_read_base: Option<&Path>,
    no_sandbox: bool,
) -> Result<(
    DocumentMeta,
    TypedWriter<Request>,
    TypedReader<Response>,
    ChildProcess,
)> {
    let (tx, mut rx, child) = fork_renderer(params, sandbox_read_base, no_sandbox)?;

    // Read metadata (first message from child)
    let meta = match rx.recv().context("failed to receive metadata from child")? {
        Response::Meta(m) => m,
        Response::Error(e) => anyhow::bail!("child build error: {e}"),
        Response::Tile(_) => anyhow::bail!("unexpected Tile response, expected Meta"),
    };

    Ok((meta, tx, rx, child))
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
