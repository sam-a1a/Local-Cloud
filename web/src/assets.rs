//! The page, served from memory.
//!
//! The whole bundle is a few tens of kilobytes and it never changes while the
//! process runs, so it is read once at startup and handed out from a map. No
//! filesystem call per request, no chance of serving half a file that Vite is
//! in the middle of rewriting, and no directory to ship alongside the binary
//! beyond the one that was built.

use anyhow::{Context, Result};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::Shared;

pub struct Asset {
    body: Bytes,
    content_type: &'static str,
    /// Vite puts a hash of the contents in the name of everything under
    /// `assets/`, so those can be cached forever and the rest not at all.
    immutable: bool,
}

pub type Assets = HashMap<String, Asset>;

/// Where `npm run build` left the page.
///
/// `CARGO_MANIFEST_DIR` rather than the working directory, so `cargo run -p web`
/// finds it from anywhere in the workspace.
fn dist() -> PathBuf {
    match std::env::var_os("LOCALCLOUD_UI") {
        Some(path) => PathBuf::from(path),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/dist"),
    }
}

pub fn load() -> Result<Assets> {
    let root = dist();
    if !root.join("index.html").is_file() {
        anyhow::bail!(
            "the page has not been built.\n  cd web/ui && npm install && npm run build\n  (looked in {})",
            root.display()
        );
    }

    let mut assets = Assets::new();
    collect(&root, &root, &mut assets).context("reading the built page")?;
    Ok(assets)
}

fn collect(root: &Path, directory: &Path, into: &mut Assets) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, into)?;
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let key = relative.to_string_lossy().replace('\\', "/");
        let body = Bytes::from(std::fs::read(&path)?);
        into.insert(
            key.clone(),
            Asset {
                content_type: content_type(&key),
                immutable: key.starts_with("assets/"),
                body,
            },
        );
    }
    Ok(())
}

/// Hand-written rather than a crate for it. This serves one bundler's output,
/// and the list of things Vite emits is this short.
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

pub async fn serve(State(app): State<Shared>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // One page, so anything that is not a file is the page. There are no
    // client-side routes yet, and when there are, this is already right.
    let asset = app
        .assets
        .get(path)
        .or_else(|| app.assets.get("index.html"));

    let Some(asset) = asset else {
        return (StatusCode::NOT_FOUND, "not built").into_response();
    };

    let cache = if asset.immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    (
        [
            (header::CONTENT_TYPE, asset.content_type),
            (header::CACHE_CONTROL, cache),
        ],
        Body::from(asset.body.clone()),
    )
        .into_response()
}
