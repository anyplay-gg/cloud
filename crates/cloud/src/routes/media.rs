//! Screenshots and clips in messages: upload into a conversation, download by a member, and the
//! moderators' read of a file a report holds. See [`crate::media`] for the rules on the file and
//! network `docs/adr/0031` for the decision.
//!
//! Both file routes are outside the request timeout of the guard in [`super`]: an upload of a
//! clip takes longer than a request should. They have a deadline of their own, a cap of
//! [`media::UPLOAD_SLOTS`] uploads per node, and every rule of a message is checked before the
//! first byte is read.

use anyplay_api::{ConversationKind, ErrorCode, Event, MEDIA_KEEP_SECS, MediaKind, Message};
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use futures_util::StreamExt as _;
use tokio::io::AsyncWriteExt as _;

use crate::chat::{self, ACCEPTED, DECLINED};
use crate::error::{AppError, Result};
use crate::http::Auth;
use crate::live::Conn;
use crate::media::{self, MediaStore as _, NewMedia, Refused};
use crate::moderation::access::Staff;
use crate::moderation::ensure_not_muted;
use crate::state::AppState;
use crate::util::{now_ms, random_uuid};

/// Removes a temporary file unless it was moved into place.
struct Temp(Option<std::path::PathBuf>);

impl Temp {
    fn path(&self) -> &std::path::Path {
        self.0.as_deref().unwrap_or(std::path::Path::new(""))
    }

    fn keep(mut self) -> std::path::PathBuf {
        self.0.take().unwrap_or_default()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The rules for a file between two accounts, checked before and again inside the transaction:
/// a direct conversation that both accepted, between friends, with no block. Returns the other
/// account.
async fn check_pair(
    conn: &mut Conn,
    conversation: &chat::ConversationRow,
    members: &[chat::MemberRow],
    me: u64,
) -> Result<u64> {
    let mine = members
        .iter()
        .find(|m| m.id() == me && m.state != DECLINED)
        .ok_or_else(AppError::not_found)?;
    if conversation.kind() != ConversationKind::Direct {
        return Err(AppError::forbidden(
            "Screenshots and clips can be sent to one friend at a time.",
        ));
    }
    let other = members
        .iter()
        .find(|m| m.id() != me)
        .ok_or_else(AppError::not_found)?;
    if mine.state != ACCEPTED || other.state != ACCEPTED {
        return Err(AppError::forbidden(
            "Screenshots and clips can only be sent to a friend.",
        ));
    }
    if chat::blocked_either(conn, me, other.id()).await? {
        return Err(AppError::forbidden("You cannot send this to this account."));
    }
    if !chat::are_friends(conn, me, other.id()).await? {
        return Err(AppError::forbidden(
            "Screenshots and clips can only be sent to a friend.",
        ));
    }
    Ok(other.id())
}

/// The `Content-Type` of an upload as one of the three types the cloud takes.
fn declared_type(request: &Request) -> Result<(&'static str, MediaKind)> {
    let text = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let content_type = match text.as_str() {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "video/mp4" => "video/mp4",
        _ => {
            return Err(AppError::bad_request(
                "Send a PNG or JPEG screenshot or an MP4 clip.",
            ));
        }
    };
    let kind = MediaKind::from_content_type(content_type).ok_or_else(AppError::internal)?;
    Ok((content_type, kind))
}

fn too_large(kind: MediaKind) -> AppError {
    AppError::too_large(match kind {
        MediaKind::Clip => "A clip may be at most 100 MB.",
        _ => "A screenshot may be at most 10 MB.",
    })
}

impl From<Refused> for AppError {
    fn from(refused: Refused) -> Self {
        match refused {
            Refused::Type => {
                AppError::bad_request("The file is not the kind of file it says it is.")
            }
            Refused::Size => {
                AppError::too_large("The picture is too large, or the clip longer than 60 seconds.")
            }
            Refused::Broken => AppError::bad_request("The file cannot be read."),
        }
    }
}

/// Reads the body into the temporary file, counting bytes against `max`. Stops at once at the
/// first byte over.
async fn receive(
    request: Request,
    temp: &std::path::Path,
    max: u64,
    kind: MediaKind,
) -> Result<u64> {
    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|_| AppError::internal())?;
    let mut stream = request.into_body().into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AppError::bad_request("The upload was cut off."))?;
        total += chunk.len() as u64;
        if total > max {
            return Err(too_large(kind));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| AppError::internal())?;
    }
    file.flush().await.map_err(|_| AppError::internal())?;
    if total == 0 {
        return Err(AppError::bad_request("The file is empty."));
    }
    Ok(total)
}

pub async fn upload(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Message>> {
    ensure_not_muted(&state, auth.account).await?;
    let (content_type, kind) = declared_type(&request)?;
    let max = kind.max_bytes();
    let announced = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if announced.is_some_and(|n| n > max) {
        return Err(too_large(kind));
    }
    // The rules first, with the lightest reads, before a single byte of the file is accepted.
    {
        let mut conn = state.db.acquire().await?;
        let conversation = chat::conversation(&mut conn, &id)
            .await?
            .ok_or_else(AppError::not_found)?;
        let members = chat::members(&mut conn, &id).await?;
        check_pair(&mut conn, &conversation, &members, auth.account).await?;
        let used = media::active_bytes(&mut conn, auth.account, now_ms()).await?;
        if used + announced.unwrap_or(0) > state.config.media_account_bytes {
            return Err(AppError::limit(
                "You have sent as many screenshots and clips as the cloud keeps for you. \
                 Unsend some, or wait until they expire.",
            ));
        }
    }
    let rates = &state.tuning.rates;
    state
        .limiter
        .hit(
            &format!("media:{}", auth.account),
            rates.media_uploads,
            rates.media_window,
        )
        .map_err(AppError::rate_limited)?;
    let _slot = media::UPLOADS
        .try_acquire()
        .map_err(|_| AppError::rate_limited(10))?;

    let store = state.media_store();
    let temp = Temp(Some(store.temp_path().map_err(|_| AppError::internal())?));
    let received = tokio::time::timeout(
        media::UPLOAD_DEADLINE,
        receive(request, temp.path(), max, kind),
    )
    .await
    .map_err(|_| AppError::bad_request("The upload took too long."))??;
    let meta = media::inspect(temp.path().to_path_buf(), content_type).await?;

    let media_id = random_uuid();
    let created_ms = now_ms();
    store
        .put(&media_id, &temp.keep())
        .map_err(|_| AppError::internal())?;
    // From here the file exists under its key; a failure below deletes it again.
    let stored = send(
        &state,
        auth.account,
        &id,
        &media_id,
        kind,
        content_type,
        received,
        meta,
        created_ms,
    )
    .await;
    let (message, told) = match stored {
        Ok(done) => done,
        Err(error) => {
            let _ = store.delete(&media_id);
            return Err(error);
        }
    };
    for (account, announce) in told {
        state.news.push(
            account,
            &Event::Message {
                conversation: id.clone(),
                seq: message.seq,
            },
        );
        if announce {
            crate::push::notify(
                &state,
                account,
                crate::push::Note::message(&message.sender.handle, &id),
            );
        }
    }
    Ok(Json(message))
}

/// Stores the message and the row in one transaction. Returns the message as its writer reads
/// it, and whom to tell, with whether to notify them.
#[allow(clippy::too_many_arguments)]
async fn send(
    state: &AppState,
    me: u64,
    id: &str,
    media_id: &str,
    kind: MediaKind,
    content_type: &str,
    bytes: u64,
    meta: media::Meta,
    created_ms: i64,
) -> Result<(Message, Vec<(u64, bool)>)> {
    let mut tx = state.begin_write().await?;
    let conversation = chat::lock_conversation(&mut tx, id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let members = chat::members(&mut tx, id).await?;
    let other = check_pair(&mut tx, &conversation, &members, me).await?;
    let from_seq = members
        .iter()
        .find(|m| m.id() == me)
        .map_or(0, |m| m.from_seq.max(0) as u64);
    let other_muted = members.iter().any(|m| m.id() == other && m.muted == 1);
    let seq = chat::next_seq(&mut tx, id, created_ms).await?;
    chat::insert_message(&mut tx, id, seq, me, "", created_ms, None).await?;
    media::insert(
        &mut tx,
        &NewMedia {
            id: media_id,
            conversation: id,
            seq,
            uploader: me,
            kind,
            content_type,
            bytes,
            meta,
            created_ms,
            expires_ms: created_ms + (MEDIA_KEEP_SECS * 1000) as i64,
        },
    )
    .await?;
    chat::set_read(&mut tx, id, me, seq).await?;
    tx.commit().await?;
    let mut conn = state.db.acquire().await?;
    let message = chat::messages_after(&mut conn, id, me, from_seq, seq - 1, 1)
        .await?
        .into_iter()
        .next()
        .filter(|m| m.seq == seq)
        .ok_or_else(AppError::internal)?;
    Ok((message, vec![(me, false), (other, !other_muted)]))
}

/// A file, to a member of its conversation who may still see its message.
pub async fn download(
    State(state): State<AppState>,
    auth: Auth,
    Path(media_id): Path<String>,
) -> Result<Response> {
    let now = now_ms();
    let mut conn = state.db.acquire().await?;
    let row = media::row(&mut conn, &media_id)
        .await?
        .filter(|r| r.is_live(now))
        .ok_or_else(AppError::not_found)?;
    let members = chat::members(&mut conn, &row.conversation_id).await?;
    let me = members
        .iter()
        .find(|m| m.id() == auth.account && m.state != DECLINED)
        .ok_or_else(AppError::not_found)?;
    let seq = row.seq as u64;
    let stored = chat::stored_message(&mut conn, &row.conversation_id, seq)
        .await?
        .filter(|m| !m.deleted())
        .ok_or_else(AppError::not_found)?;
    let _ = stored;
    if row.seq <= me.from_seq
        || chat::hidden_from(&mut conn, &row.conversation_id, seq)
            .await?
            .contains(&auth.account)
    {
        return Err(AppError::not_found());
    }
    drop(conn);
    file_response(&state, &row)
}

/// A held file, to a moderator.
pub async fn moderate(
    State(state): State<AppState>,
    _staff: Staff,
    Path(media_id): Path<String>,
) -> Result<Response> {
    let row = media::row(&mut *state.db.acquire().await?, &media_id)
        .await?
        .filter(|r| r.is_held(now_ms()))
        .ok_or_else(AppError::not_found)?;
    file_response(&state, &row)
}

fn file_response(state: &AppState, row: &media::MediaRow) -> Result<Response> {
    let file = state.media_store().open(&row.id).map_err(|error| {
        tracing::error!(%error, "a media file is missing");
        AppError::new(ErrorCode::NotFound, "The file is gone.")
    })?;
    let file = tokio::fs::File::from_std(file);
    let stream = futures_util::stream::unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt as _;
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(buf)), file))
            }
            Err(error) => Some((Err(error), file)),
        }
    });
    let mut response = Body::from_stream(stream).into_response();
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    let fixed = |text: &str| HeaderValue::from_str(text).unwrap_or(HeaderValue::from_static(""));
    headers.insert(header::CONTENT_TYPE, fixed(&row.content_type));
    headers.insert(header::CONTENT_LENGTH, fixed(&row.bytes.to_string()));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    if row.kind() == MediaKind::Clip {
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"clip.mp4\""),
        );
    }
    Ok(response)
}

/// Used by the export: which media an account sent.
pub async fn export(state: &AppState, account: u64) -> Result<Vec<serde_json::Value>> {
    let rows = media::sent_by(&mut *state.db.acquire().await?, account).await?;
    Ok(rows
        .into_iter()
        .map(|(id, kind, created)| serde_json::json!({"id": id, "kind": kind, "created": created}))
        .collect())
}
