//! Choosing where this device keeps what it holds.
//!
//! A browser cannot hand a server a directory. `webkitdirectory` gives up file
//! names and no path, and there is no picker that returns one - which is
//! correct of the browser and unhelpful here, because the folder being chosen
//! belongs to the machine the engine is on. So the choosing happens on this
//! side: the page asks what is in a directory and shows it, and sends back the
//! path it settled on.
//!
//! Listing directories over an API is not a new exposure. This one already
//! hands back the contents of any file in the catalog and unpairs devices; it
//! is bound to loopback for that reason and this changes nothing about it.

use crate::api::{ApiError, ApiResult};
use crate::{Shared, settings};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub path: String,
    /// Absent at the root, which is the only place with nowhere above it.
    pub parent: Option<String>,
    pub folders: Vec<Folder>,
    /// How many files are already sitting here. Choosing a folder puts them in
    /// the mesh, so the number belongs on screen before the decision, not after.
    pub files: usize,
}

#[derive(Serialize)]
pub struct Folder {
    pub name: String,
    pub path: String,
}

#[derive(Deserialize)]
pub struct Where {
    path: Option<String>,
}

fn bad(reason: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, reason.into())
}

/// What is inside a directory, for a page that is drawing a chooser.
pub async fn browse(State(app): State<Shared>, Query(query): Query<Where>) -> ApiResult<Json<Listing>> {
    let start = match query.path {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        // Home, because that is where a person's folders are, rather than the
        // root, which is where the machine's are.
        _ => PathBuf::from(app.engine().sync_dir()),
    };

    tokio::task::spawn_blocking(move || read(&start))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

fn read(directory: &Path) -> ApiResult<Json<Listing>> {
    let directory = directory
        .canonicalize()
        .map_err(|_| bad(format!("{} is not a folder that exists.", directory.display())))?;

    let mut folders = Vec::new();
    let mut files = 0;

    let entries = std::fs::read_dir(&directory)
        .map_err(|e| bad(format!("{} cannot be read: {e}", directory.display())))?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Hidden entries are hidden here for the same reason the indexer skips
        // them: they are the machine's bookkeeping, not a place to keep files.
        if name.starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => folders.push(Folder {
                path: entry.path().to_string_lossy().to_string(),
                name,
            }),
            Ok(_) => files += 1,
            Err(_) => {}
        }
    }
    folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(Json(Listing {
        parent: directory
            .parent()
            .map(|p| p.to_string_lossy().to_string()),
        path: directory.to_string_lossy().to_string(),
        folders,
        files,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewFolder {
    parent: String,
    name: String,
}

/// Somewhere to put things often does not exist yet.
pub async fn create(State(_app): State<Shared>, Json(body): Json<NewFolder>) -> ApiResult<Json<Listing>> {
    let name = body.name.trim().to_string();
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(bad("That is not a usable folder name."));
    }
    let path = PathBuf::from(&body.parent).join(&name);
    std::fs::create_dir_all(&path).map_err(|e| bad(format!("Could not create it: {e}")))?;
    read(&path)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chosen {
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub sync_dir: String,
    /// Files carried over from the folder that was in use.
    pub moved: usize,
    /// Files left behind because something of the same name was already at the
    /// destination. Nothing is overwritten to make room.
    pub left_behind: usize,
    /// Files that were already in the chosen folder and have now joined the
    /// mesh.
    pub adopted: u32,
}

/// Moves this device to another folder.
///
/// The catalog records paths relative to the sync folder, so moving the folder
/// is invisible to it - as long as the files move too. That is the whole reason
/// this is one operation rather than a setting: a changed path with the files
/// left where they were would leave the engine certain it holds things it
/// cannot find, and it would say so to every other device in the mesh.
pub async fn choose(State(app): State<Shared>, Json(body): Json<Chosen>) -> ApiResult<Json<Outcome>> {
    // One at a time. Two of these overlapping would stop one engine, start
    // another, and leave the third holding a folder nobody is watching.
    let _turn = app.switching.lock().await;

    let shared = app.clone();
    tokio::task::spawn_blocking(move || switch(&shared, &body.path))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .inspect(|_| app.changed.notify_one())
        .map(Json)
}

fn switch(app: &Shared, wanted: &str) -> ApiResult<Outcome> {
    let engine = app.engine();
    let current = PathBuf::from(engine.sync_dir());

    let wanted = PathBuf::from(wanted);
    if !wanted.is_absolute() {
        return Err(bad("Give a full path."));
    }
    std::fs::create_dir_all(&wanted).map_err(|e| bad(format!("Could not use that folder: {e}")))?;
    let wanted = wanted
        .canonicalize()
        .map_err(|e| bad(format!("Could not use that folder: {e}")))?;

    if wanted == current {
        return Ok(Outcome {
            sync_dir: current.to_string_lossy().to_string(),
            moved: 0,
            left_behind: 0,
            adopted: 0,
        });
    }
    if wanted.parent().is_none() {
        return Err(bad("The root of the disk is not a sync folder."));
    }
    // The engine's own state lives under the base directory. A sync folder
    // containing it would mean watching, hashing and offering the device's
    // private key to the mesh.
    let state = PathBuf::from(&app.base_dir);
    if state.starts_with(&wanted) || wanted.starts_with(&state) {
        return Err(bad("That folder holds this device's own state. Choose another."));
    }
    if wanted.starts_with(&current) || current.starts_with(&wanted) {
        return Err(bad("That folder is inside the current one, or contains it."));
    }

    // Stopped before anything moves: the watcher is looking at the old folder,
    // and files leaving under it would read as deletions.
    engine.stop();

    let (moved, left_behind) = move_contents(&current, &wanted)
        .map_err(|e| bad(format!("Could not move the files across: {e}")))?;

    let opened = app.open(&wanted.to_string_lossy());
    let opened = match opened {
        Ok(engine) => engine,
        Err(problem) => {
            // Put it back rather than leave the device stopped. The files have
            // already moved, so the old engine is restarted on the new folder
            // if it can be, and on the old one if not.
            let _ = app.open(&current.to_string_lossy());
            return Err(bad(problem));
        }
    };

    let adopted = opened.scan_sync_folder();

    settings::save(
        &app.base_dir,
        &settings::Settings {
            sync_dir: Some(wanted.to_string_lossy().to_string()),
        },
    )
    .map_err(|e| bad(format!("The folder is in use, but could not be remembered: {e}")))?;

    Ok(Outcome {
        sync_dir: wanted.to_string_lossy().to_string(),
        moved,
        left_behind,
        adopted,
    })
}

/// Carries everything from one folder to another, and says what it could not.
///
/// Nothing is overwritten. A name already taken at the destination means the
/// file stays where it is, and the count comes back so the page can say so -
/// silently replacing somebody's file to tidy up a move is not a trade this
/// gets to make.
fn move_contents(from: &Path, to: &Path) -> std::io::Result<(usize, usize)> {
    let mut moved = 0;
    let mut left = 0;

    for entry in std::fs::read_dir(from)?.flatten() {
        let source = entry.path();
        let Some(name) = source.file_name() else {
            continue;
        };
        let destination = to.join(name);
        if destination.exists() {
            left += 1;
            continue;
        }
        // A rename is instant within a disk and impossible across one, which is
        // exactly the case that matters here - a folder on an external drive.
        match std::fs::rename(&source, &destination) {
            Ok(()) => moved += 1,
            Err(_) => {
                copy_across(&source, &destination)?;
                if source.is_dir() {
                    std::fs::remove_dir_all(&source)?;
                } else {
                    std::fs::remove_file(&source)?;
                }
                moved += 1;
            }
        }
    }
    Ok((moved, left))
}

fn copy_across(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)?.flatten() {
            copy_across(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(source, destination).map(|_| ())
}
