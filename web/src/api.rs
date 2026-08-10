//! Everything the page can ask this device to do.
//!
//! Every engine call blocks - the API returns as soon as a request is
//! understood and finishes the network part in the background, but "as soon as"
//! still means a database write on the calling thread - so none of them happen
//! on a thread that is serving requests. `blocking` is the whole of that rule.
//!
//! The errors are better here than in either app. `EngineError` writes a
//! sentence for every variant and `Display` produces it; it is only crossing
//! the FFI that loses it, and nothing here crosses the FFI.

use crate::Shared;
use crate::snapshot;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use localcloud::{CollisionResolution, Engine};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

/// A failure, as something the page can put beside the row that caused it.
pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorBody { error: self.1 })).into_response()
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl From<localcloud::EngineError> for ApiError {
    /// The engine's own sentence, unchanged. It wrote one for every variant and
    /// it is better than anything this layer would invent.
    fn from(error: localcloud::EngineError) -> Self {
        ApiError(StatusCode::BAD_REQUEST, error.to_string())
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Runs one engine call somewhere that is allowed to block.
async fn blocking<T, F>(app: &Shared, work: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Engine) -> Result<T, localcloud::EngineError> + Send + 'static,
{
    let engine = app.engine();
    let result = tokio::task::spawn_blocking(move || work(&engine))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // A call that failed part way through may still have changed something, so
    // the page is told to look either way.
    app.changed.notify_one();
    Ok(result?)
}

#[derive(Serialize)]
struct Ok_ {
    ok: bool,
}

fn done() -> Json<impl Serialize> {
    Json(Ok_ { ok: true })
}

// -- Reading -----------------------------------------------------------------

/// The whole state, for a page that has just loaded and has nothing yet.
///
/// Afterwards it arrives over `/api/events` instead, and only when it differs
/// from what was last sent.
pub async fn state(State(app): State<Shared>) -> ApiResult<Json<snapshot::Snapshot>> {
    let engine = app.engine();
    let snapshot = tokio::task::spawn_blocking(move || snapshot::read(&engine))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(snapshot))
}

/// Hands a file to the browser.
///
/// Streamed from disk rather than read into memory: a copy that arrived here is
/// a whole file, and some of them are films.
pub async fn download(State(app): State<Shared>, Path(id): Path<String>) -> ApiResult<Response> {
    let engine = app.engine();
    let wanted = id.clone();
    let found = tokio::task::spawn_blocking(move || {
        engine
            .local_files()
            .into_iter()
            .find(|meta| meta.id == wanted)
            .map(|meta| (format!("{}/{}", engine.sync_dir(), meta.path), meta.path))
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((path, relative)) = found else {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "This device does not have that file's contents.".into(),
        ));
    };

    let file = tokio::fs::File::open(&path).await?;
    let length = file.metadata().await?.len();
    let stream = tokio_util::io::ReaderStream::new(file);

    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, length.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename*=UTF-8''{}",
                    urlencode(&snapshot::file_name(&relative))
                ),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

// -- Writing -----------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportQuery {
    name: String,
}

/// Takes a file from the browser into the mesh.
///
/// The body is the file, streamed straight to disk. Not multipart, and not
/// buffered: a multipart parser would hold the parts in memory and this is the
/// one endpoint that can be handed a gigabyte.
///
/// It lands in a temporary file first because `import_file` takes a path and
/// copies it into the sync folder - writing it into the folder directly would
/// race the watcher, which would index a half-written file.
pub async fn import(
    State(app): State<Shared>,
    Query(query): Query<ImportQuery>,
    body: Body,
) -> ApiResult<Json<impl Serialize>> {
    let staging = std::env::temp_dir().join("localcloud-incoming");
    tokio::fs::create_dir_all(&staging).await?;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let temporary = staging.join(format!("{}-{unique}", std::process::id()));

    let mut file = tokio::fs::File::create(&temporary).await?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);

    let source = temporary.to_string_lossy().to_string();
    let name = query.name.clone();
    let outcome = blocking(&app, move |engine| {
        engine.import_file(source, name).map(|_| ())
    })
    .await;

    tokio::fs::remove_file(&temporary).await.ok();
    outcome?;
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    file_id: String,
    device_ids: Vec<String>,
}

pub async fn share(
    State(app): State<Shared>,
    Json(body): Json<Share>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| {
        engine.share_to(body.file_id, body.device_ids)
    })
    .await?;
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    file_id: String,
}

pub async fn pull(
    State(app): State<Shared>,
    Json(body): Json<FileRef>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| engine.pull_copy(body.file_id)).await?;
    Ok(done())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    remaining_copies: i64,
    trashed: bool,
}

pub async fn delete_here(
    State(app): State<Shared>,
    Json(body): Json<FileRef>,
) -> ApiResult<Json<Deleted>> {
    let outcome = blocking(&app, move |engine| {
        engine.delete_local_copy(body.file_id)
    })
    .await?;
    Ok(Json(Deleted {
        remaining_copies: outcome.remaining_copies,
        trashed: outcome.trashed,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    file_id: String,
    device_id: String,
}

pub async fn delete_copy(
    State(app): State<Shared>,
    Json(body): Json<Copy>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| {
        engine.delete_copy(body.file_id, body.device_id)
    })
    .await?;
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRef {
    device_id: String,
}

#[derive(Serialize)]
pub struct Code {
    code: String,
}

pub async fn pair_start(
    State(app): State<Shared>,
    Json(body): Json<DeviceRef>,
) -> ApiResult<Json<Code>> {
    let code = blocking(&app, move |engine| {
        engine.start_pairing(vec![body.device_id])
    })
    .await?;
    Ok(Json(Code { code }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirm {
    device_id: String,
    code: String,
}

pub async fn pair_confirm(
    State(app): State<Shared>,
    Json(body): Json<Confirm>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| {
        engine.confirm_pairing(body.device_id, body.code)
    })
    .await?;
    Ok(done())
}

pub async fn pair_cancel(State(app): State<Shared>) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| {
        engine.cancel_pairing();
        Ok(())
    })
    .await?;
    Ok(done())
}

pub async fn unpair(
    State(app): State<Shared>,
    Json(body): Json<DeviceRef>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| engine.unpair(body.device_id)).await?;
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Name {
    name: String,
}

pub async fn rename(
    State(app): State<Shared>,
    Json(body): Json<Name>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| engine.set_device_name(body.name)).await?;
    Ok(done())
}

pub async fn restore(
    State(app): State<Shared>,
    Json(body): Json<FileRef>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| engine.restore_file(body.file_id)).await?;
    Ok(done())
}

pub async fn destroy(
    State(app): State<Shared>,
    Json(body): Json<FileRef>,
) -> ApiResult<Json<impl Serialize>> {
    blocking(&app, move |engine| engine.delete_permanently(body.file_id)).await?;
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolve {
    collision_id: String,
    /// True keeps both items, which is what the engine already did; false gives
    /// the name to the one that arrived and trashes the other.
    keep_both: bool,
}

pub async fn resolve_collision(
    State(app): State<Shared>,
    Json(body): Json<Resolve>,
) -> ApiResult<Json<impl Serialize>> {
    let resolution = if body.keep_both {
        CollisionResolution::KeepBoth
    } else {
        CollisionResolution::Override
    };
    blocking(&app, move |engine| {
        engine.resolve_collision(body.collision_id, resolution)
    })
    .await?;
    Ok(done())
}

/// Percent-encodes a filename for a `Content-Disposition` header.
///
/// Everything outside the unreserved set, because a name can contain a quote, a
/// semicolon or a newline, and a header is not a place to find out.
fn urlencode(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for byte in name.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
