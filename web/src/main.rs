//! A browser cannot join the mesh, so this joins it on the browser's behalf.
//!
//! The peer protocol is HTTPS with mutual TLS against certificates pinned by
//! pairing, on a port found over mDNS. A browser will not present a client
//! certificate from a store it does not have, will not accept a self-signed
//! server certificate, and cannot see mDNS at all. None of that is an oversight
//! in the browser; it is what the protocol is for.
//!
//! So this process is the mesh member. It runs a whole engine - it pairs, it is
//! discovered, it holds copies - and puts a plain HTTP API in front of it for a
//! page served from the same place. The page is a view of this device. It is
//! not a peer, and nothing about the mesh had to change to give it one.
//!
//!     cd web/ui && npm install && npm run build
//!     cargo run -p web
//!     open http://127.0.0.1:7777

mod api;
mod assets;
mod events;
mod snapshot;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::{get, post};
use localcloud::Engine;
use std::sync::Arc;
use tokio::sync::{Notify, broadcast};

/// Loopback, always.
///
/// Not a default to be overridden: this API unpairs devices, deletes copies off
/// other machines and hands back the contents of any file in the catalog, with
/// no authentication of any kind. It is safe only because the operating system
/// will not route it off this machine. The mesh port - the one that *is* on the
/// network - is chosen by the engine and defended by mutual TLS.
const BIND: &str = "127.0.0.1:7777";

pub struct App {
    engine: Arc<Engine>,
    /// Serialised once, fanned out to whatever tabs are open.
    updates: broadcast::Sender<Arc<events::Message>>,
    /// Rung when something happened that a snapshot would show.
    changed: Arc<Notify>,
    assets: assets::Assets,
}

pub type Shared = Arc<App>;

fn main() -> Result<()> {
    // The engine builds a runtime of its own and starts before this one exists,
    // which is also the order the bindings use: construct, listen, start.
    let (base_dir, sync_dir) = directories()?;
    let engine = Arc::new(
        Engine::new(base_dir.clone(), sync_dir.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("starting the engine")?,
    );

    let (updates, _) = broadcast::channel(64);
    let changed = Arc::new(Notify::new());

    engine.set_event_listener(Arc::new(events::Bridge::new(
        updates.clone(),
        changed.clone(),
    )));
    engine.start().map_err(|e| anyhow::anyhow!("{e}"))?;

    println!(
        "\n{} · {} · {}\nState:  {}\nFiles:  {}\n",
        engine.device_name(),
        engine.device_platform(),
        &engine.device_id()[..8],
        base_dir,
        sync_dir,
    );

    let app = Arc::new(App {
        engine,
        updates,
        changed,
        assets: assets::load()?,
    });

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve(app))
}

async fn serve(app: Shared) -> Result<()> {
    tokio::spawn(snapshot::broadcast_changes(app.clone()));

    let router = Router::new()
        .route("/api/events", get(events::stream))
        .route("/api/state", get(api::state))
        .route("/api/import", post(api::import))
        .route("/api/file/{id}", get(api::download))
        .route("/api/share", post(api::share))
        .route("/api/pull", post(api::pull))
        .route("/api/delete-here", post(api::delete_here))
        .route("/api/delete-copy", post(api::delete_copy))
        .route("/api/pair/start", post(api::pair_start))
        .route("/api/pair/confirm", post(api::pair_confirm))
        .route("/api/pair/cancel", post(api::pair_cancel))
        .route("/api/unpair", post(api::unpair))
        .route("/api/rename", post(api::rename))
        .route("/api/trash/restore", post(api::restore))
        .route("/api/trash/destroy", post(api::destroy))
        .route("/api/collision", post(api::resolve_collision))
        .fallback(assets::serve)
        .with_state(app.clone());

    let listener = tokio::net::TcpListener::bind(BIND)
        .await
        .with_context(|| format!("binding {BIND}"))?;

    println!("Open http://{BIND}\n");

    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        println!("\nStopping.");
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;

    app.engine.stop();
    Ok(())
}

/// Where this device keeps its state, and where it keeps its files.
///
/// The files go somewhere a person can actually find them - `~/LocalCloud` -
/// because on a desktop the engine watches that folder, so anything dropped in
/// it is indexed and offered to the mesh without the browser being involved.
/// The state goes somewhere they will not: it is a private key and a database,
/// and neither is a document.
fn directories() -> Result<(String, String)> {
    if let Some(base) = std::env::args().nth(1) {
        let base = base.trim_end_matches('/').to_string();
        return Ok((format!("{base}/state"), format!("{base}/files")));
    }

    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok((format!("{home}/.localcloud"), format!("{home}/LocalCloud")))
}
