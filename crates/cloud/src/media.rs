//! Screenshots and clips that players send to a friend (network `spec/cloud-api.md`, "Media in
//! messages", and `docs/adr/0032`).
//!
//! A file lives in a [`MediaStore`] under a key of 128 random bits and has one row in `media`.
//! The row has no foreign key to the message: it is the only record that its file exists, so it
//! outlives the message until [`sweep`] has deleted the file. Access is checked on every read
//! against the conversation, never by the key alone.
//!
//! What is checked on a file: its type by its first bytes (not by what the request says), its
//! size while it streams in, a screenshot by decoding it under limits, a clip by reading its
//! `moov` box for the length and the picture size. The bytes are stored as they came, and are
//! served with `nosniff`, a `sandbox` content security policy and a fixed `Content-Type`.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use anyplay_api::{MAX_CLIP_SECS, MediaInfo, MediaKind};
use image::{ImageDecoder as _, ImageFormat, ImageReader, Limits};
use sqlx::FromRow;
use tokio::sync::Semaphore;

use crate::live::Conn;
use crate::state::Inner;

fn i(id: u64) -> i64 {
    id as i64
}

fn u(id: i64) -> u64 {
    id as u64
}

/// Where the files are. The cloud has one, [`DiskStore`]; the trait is the seam for an object
/// store later.
pub trait MediaStore: Send + Sync {
    /// A path in the store's own folder for an upload that is still arriving. Moving it into
    /// place with [`MediaStore::put`] is then a rename on one file system.
    fn temp_path(&self) -> std::io::Result<PathBuf>;
    /// Takes the finished temporary file as the file `key`.
    fn put(&self, key: &str, temp: &Path) -> std::io::Result<()>;
    fn open(&self, key: &str) -> std::io::Result<std::fs::File>;
    /// Deletes a file; a file that is not there is not an error.
    fn delete(&self, key: &str) -> std::io::Result<()>;
    /// Deletes temporary files older than `age`.
    fn clear_temp(&self, age: std::time::Duration);
}

/// Files in a folder, two levels deep (`ab/ab12...`), temporary files in `tmp/`.
pub struct DiskStore {
    root: PathBuf,
}

impl DiskStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path(&self, key: &str) -> std::io::Result<PathBuf> {
        // Keys are made by the cloud, but a path never trusts that.
        if key.len() < 8
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(std::io::Error::other("not a media key"));
        }
        Ok(self.root.join(&key[..2]).join(key))
    }
}

impl MediaStore for DiskStore {
    fn temp_path(&self) -> std::io::Result<PathBuf> {
        let dir = self.root.join("tmp");
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join(crate::util::random_uuid()))
    }

    fn put(&self, key: &str, temp: &Path) -> std::io::Result<()> {
        let path = self.path(key)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::rename(temp, path)
    }

    fn open(&self, key: &str) -> std::io::Result<std::fs::File> {
        std::fs::File::open(self.path(key)?)
    }

    fn delete(&self, key: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.path(key)?) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn clear_temp(&self, age: std::time::Duration) {
        let Ok(entries) = std::fs::read_dir(self.root.join("tmp")) else {
            return;
        };
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|e| e > age);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

impl Inner {
    /// The store of screenshots and clips: `media_dir`, or `data_dir/media`.
    pub fn media_store(&self) -> DiskStore {
        DiskStore::new(
            self.config
                .media_dir
                .clone()
                .unwrap_or_else(|| self.config.data_dir.join("media")),
        )
    }
}

/// How many uploads one node takes at a time. Each holds a file open and may be decoded.
pub const UPLOAD_SLOTS: usize = 4;
pub static UPLOADS: Semaphore = Semaphore::const_new(UPLOAD_SLOTS);
/// How many screenshots one node decodes at a time.
static DECODERS: Semaphore = Semaphore::const_new(2);

/// The longest a whole upload may take, from the first byte to the last.
pub const UPLOAD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// The longest an upload may go without a single byte arriving.
pub const UPLOAD_IDLE: std::time::Duration = std::time::Duration::from_secs(20);

/// What a file turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u32>,
}

/// Why a file was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not the type the request declared.
    Type,
    /// A picture with too many pixels, or a clip that is too long.
    Size,
    /// Does not read as a PNG, JPEG or MP4.
    Broken,
}

const MAX_SIDE: u32 = 8192;
const MAX_DECODE_BYTES: u64 = 96 * 1024 * 1024;

/// Checks the file at `path`, which has the declared `content_type`, and reads its size.
pub async fn inspect(path: PathBuf, content_type: &'static str) -> Result<Meta, Refused> {
    let kind = MediaKind::from_content_type(content_type).ok_or(Refused::Type)?;
    let _slot = DECODERS.acquire().await.map_err(|_| Refused::Broken)?;
    tokio::task::spawn_blocking(move || match kind {
        MediaKind::Clip => inspect_clip(&path),
        _ => inspect_image(&path, content_type),
    })
    .await
    .map_err(|_| Refused::Broken)?
}

fn inspect_image(path: &Path, content_type: &str) -> Result<Meta, Refused> {
    let bytes = std::fs::read(path).map_err(|_| Refused::Broken)?;
    let declared = match content_type {
        "image/png" => ImageFormat::Png,
        _ => ImageFormat::Jpeg,
    };
    if image::guess_format(&bytes).ok() != Some(declared) {
        return Err(Refused::Type);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = ImageReader::with_format(std::io::Cursor::new(&bytes), declared);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(refusal)?;
    if decoder.total_bytes() > MAX_DECODE_BYTES {
        return Err(Refused::Size);
    }
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(Refused::Broken);
    }
    // A full decode, so that a file that only has a good header is refused.
    image::DynamicImage::from_decoder(decoder).map_err(refusal)?;
    Ok(Meta {
        width: Some(width),
        height: Some(height),
        duration_ms: None,
    })
}

fn refusal(error: image::ImageError) -> Refused {
    match error {
        image::ImageError::Limits(_) => Refused::Size,
        image::ImageError::Unsupported(_) => Refused::Type,
        _ => Refused::Broken,
    }
}

fn inspect_clip(path: &Path) -> Result<Meta, Refused> {
    let mut file = std::fs::File::open(path).map_err(|_| Refused::Broken)?;
    let meta = read_mp4(&mut file)?;
    let duration = meta.duration_ms.unwrap_or(0);
    // Half a second of slack: a clip that was cut at 60 seconds can end a few frames over.
    if duration == 0 {
        return Err(Refused::Broken);
    }
    if duration > MAX_CLIP_SECS * 1000 + 500 {
        return Err(Refused::Size);
    }
    Ok(meta)
}

/// The largest `moov` box read into memory.
const MAX_MOOV: u64 = 16 * 1024 * 1024;
/// The most top-level boxes read before a `moov` must have turned up. A real file has a handful;
/// a file of millions of empty boxes would keep a thread seeking.
const MAX_TOP_BOXES: u32 = 1024;

/// Reads the top-level boxes of an MP4 file: it must start with `ftyp` and hold a `moov` with a
/// movie header; the length comes from `mvhd`, the picture size from the first `tkhd` with one.
pub fn read_mp4<R: std::io::Read + std::io::Seek>(file: &mut R) -> Result<Meta, Refused> {
    let end = file.seek(SeekFrom::End(0)).map_err(|_| Refused::Broken)?;
    let mut at = 0u64;
    let mut first = true;
    let mut boxes_seen = 0u32;
    while at + 8 <= end {
        file.seek(SeekFrom::Start(at))
            .map_err(|_| Refused::Broken)?;
        let mut head = [0u8; 8];
        file.read_exact(&mut head).map_err(|_| Refused::Broken)?;
        let mut size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        let kind = [head[4], head[5], head[6], head[7]];
        let mut header = 8u64;
        if size == 1 {
            let mut large = [0u8; 8];
            file.read_exact(&mut large).map_err(|_| Refused::Broken)?;
            size = u64::from_be_bytes(large);
            header = 16;
        } else if size == 0 {
            size = end - at;
        }
        if size < header || size > end - at {
            return Err(Refused::Broken);
        }
        if first && &kind != b"ftyp" {
            return Err(Refused::Type);
        }
        first = false;
        if &kind == b"moov" {
            let len = size - header;
            if len > MAX_MOOV {
                return Err(Refused::Size);
            }
            let mut moov = vec![0u8; len as usize];
            file.read_exact(&mut moov).map_err(|_| Refused::Broken)?;
            return parse_moov(&moov);
        }
        // `size <= end - at` above, so this cannot wrap and always moves forward.
        at += size;
        boxes_seen += 1;
        if boxes_seen > MAX_TOP_BOXES {
            return Err(Refused::Broken);
        }
    }
    Err(Refused::Broken)
}

/// The children of a box body as (type, body) pairs.
fn boxes(mut data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while data.len() >= 8 {
        let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let kind = [data[4], data[5], data[6], data[7]];
        let size = if size == 0 { data.len() } else { size };
        if size < 8 || size > data.len() {
            break;
        }
        out.push((kind, &data[8..size]));
        data = &data[size..];
    }
    out
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    data.get(at..at + 8)
        .map(|b| u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn parse_moov(moov: &[u8]) -> Result<Meta, Refused> {
    let mut duration_ms = None;
    let mut size = None;
    for (kind, body) in boxes(moov) {
        match &kind {
            b"mvhd" => {
                let version = *body.first().ok_or(Refused::Broken)?;
                let (timescale, duration) = if version == 1 {
                    (be32(body, 20), be64(body, 24))
                } else {
                    (be32(body, 12), be32(body, 16).map(u64::from))
                };
                let (Some(timescale), Some(duration)) = (timescale, duration) else {
                    return Err(Refused::Broken);
                };
                if timescale == 0 {
                    return Err(Refused::Broken);
                }
                let ms = duration.saturating_mul(1000) / u64::from(timescale);
                duration_ms = Some(u32::try_from(ms).unwrap_or(u32::MAX));
            }
            b"trak" if size.is_none() => {
                for (inner, body) in boxes(body) {
                    if &inner == b"tkhd" {
                        let version = body.first().copied().unwrap_or(0);
                        let at = if version == 1 { 88 } else { 76 };
                        if let (Some(w), Some(h)) = (be32(body, at), be32(body, at + 4)) {
                            let (w, h) = (w >> 16, h >> 16);
                            if w > 0 && h > 0 {
                                size = Some((w, h));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(Meta {
        width: size.map(|s| s.0),
        height: size.map(|s| s.1),
        duration_ms: Some(duration_ms.ok_or(Refused::Broken)?),
    })
}

// The rows

#[derive(Clone, Debug, FromRow)]
pub struct MediaRow {
    pub id: String,
    pub conversation_id: String,
    pub seq: i64,
    pub uploader_id: i64,
    pub kind: String,
    pub content_type: String,
    pub bytes: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub created_ms: i64,
    pub expires_ms: i64,
    pub removed_ms: Option<i64>,
    pub hold_until_ms: Option<i64>,
}

const COLUMNS: &str = "id, conversation_id, seq, uploader_id, kind, content_type, bytes, width, \
     height, duration_ms, created_ms, expires_ms, removed_ms, hold_until_ms";

fn kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        _ => "clip",
    }
}

impl MediaRow {
    pub fn kind(&self) -> MediaKind {
        match self.kind.as_str() {
            "clip" => MediaKind::Clip,
            _ => MediaKind::Image,
        }
    }

    pub fn info(&self) -> MediaInfo {
        MediaInfo {
            id: self.id.clone(),
            kind: self.kind(),
            content_type: self.content_type.clone(),
            bytes: u(self.bytes),
            width: self.width.map(|v| v as u32),
            height: self.height.map(|v| v as u32),
            duration_ms: self.duration_ms.map(|v| v as u32),
            expires: u(self.expires_ms / 1000),
        }
    }

    /// Members may read it: not removed, not expired.
    pub fn is_live(&self, now_ms: i64) -> bool {
        self.removed_ms.is_none() && self.expires_ms > now_ms
    }

    pub fn is_held(&self, now_ms: i64) -> bool {
        self.hold_until_ms.is_some_and(|t| t > now_ms)
    }
}

pub struct NewMedia<'a> {
    pub id: &'a str,
    pub conversation: &'a str,
    pub seq: u64,
    pub uploader: u64,
    pub kind: MediaKind,
    pub content_type: &'a str,
    pub bytes: u64,
    pub meta: Meta,
    pub created_ms: i64,
    pub expires_ms: i64,
}

pub async fn insert(conn: &mut Conn, media: &NewMedia<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO media (id, conversation_id, seq, uploader_id, kind, content_type, bytes, \
         width, height, duration_ms, created_ms, expires_ms) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(media.id)
    .bind(media.conversation)
    .bind(i(media.seq))
    .bind(i(media.uploader))
    .bind(kind_name(media.kind))
    .bind(media.content_type)
    .bind(i(media.bytes))
    .bind(media.meta.width.map(i64::from))
    .bind(media.meta.height.map(i64::from))
    .bind(media.meta.duration_ms.map(i64::from))
    .bind(media.created_ms)
    .bind(media.expires_ms)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn row(conn: &mut Conn, id: &str) -> sqlx::Result<Option<MediaRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM media WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
}

pub async fn of_message(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
) -> sqlx::Result<Option<MediaRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM media WHERE conversation_id = $1 AND seq = $2"
    )))
    .bind(conversation)
    .bind(i(seq))
    .fetch_optional(conn)
    .await
}

/// The bytes of the files an account sent that are still on disk. An unsent or expired file
/// counts until the sweep has deleted it (an unsent one is held for a report first), so that
/// sending and unsending cannot fill the disk past the quota.
pub async fn stored_bytes(conn: &mut Conn, account: u64) -> sqlx::Result<u64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(CAST(SUM(bytes) AS BIGINT), 0) FROM media WHERE uploader_id = $1",
    )
    .bind(i(account))
    .fetch_one(conn)
    .await?;
    Ok(u(n))
}

/// Puts the `media` field on the messages of a fetch that carry a live file.
pub async fn attach(
    conn: &mut Conn,
    conversation: &str,
    messages: &mut [anyplay_api::Message],
    now_ms: i64,
) -> sqlx::Result<()> {
    let (Some(low), Some(high)) = (
        messages.iter().map(|m| m.seq).min(),
        messages.iter().map(|m| m.seq).max(),
    ) else {
        return Ok(());
    };
    let rows: Vec<MediaRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM media WHERE conversation_id = $1 AND seq >= $2 AND seq <= $3 \
         AND removed_ms IS NULL AND expires_ms > $4"
    )))
    .bind(conversation)
    .bind(i(low))
    .bind(i(high))
    .bind(now_ms)
    .fetch_all(conn)
    .await?;
    for row in rows {
        if let Some(message) = messages.iter_mut().find(|m| m.seq == u(row.seq)) {
            message.media = Some(row.info());
        }
    }
    Ok(())
}

/// The sender unsent the message: nobody reads the file any more. It is kept `hold_secs` for a
/// report, like the text of an unsent message, and then the sweep deletes it.
pub async fn unsent(
    state: &Inner,
    conversation: &str,
    seq: u64,
    hold_secs: u64,
) -> sqlx::Result<()> {
    let now = crate::util::now_ms();
    let mut conn = state.db.acquire().await?;
    sqlx::query(
        "UPDATE media SET removed_ms = $3, hold_until_ms = $4 \
         WHERE conversation_id = $1 AND seq = $2 AND removed_ms IS NULL",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(now)
    .bind(now + (hold_secs * 1000) as i64)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A report names the message: its file stays until `until_ms` for the moderators.
pub async fn hold(
    conn: &mut Conn,
    conversation: &str,
    seq: u64,
    until_ms: i64,
) -> sqlx::Result<Option<MediaRow>> {
    sqlx::query(
        "UPDATE media SET hold_until_ms = $3 WHERE conversation_id = $1 AND seq = $2 \
         AND (hold_until_ms IS NULL OR hold_until_ms < $3)",
    )
    .bind(conversation)
    .bind(i(seq))
    .bind(until_ms)
    .execute(&mut *conn)
    .await?;
    of_message(conn, conversation, seq).await
}

/// For the export: what the account sent, without the bytes.
pub async fn sent_by(conn: &mut Conn, account: u64) -> sqlx::Result<Vec<(String, String, u64)>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT id, kind, created_ms FROM media WHERE uploader_id = $1 ORDER BY created_ms",
    )
    .bind(i(account))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, kind, created)| (id, kind, u(created / 1000)))
        .collect())
}

impl Inner {
    /// Deletes the files and rows that are expired or unsent and not held, a batch at a time,
    /// and the half-uploaded files left behind. Safe to run on every node at once: a file that
    /// is gone already is not an error.
    pub async fn sweep_media(&self, now_ms: i64) -> sqlx::Result<u64> {
        const BATCH: i64 = 200;
        let store = self.media_store();
        store.clear_temp(std::time::Duration::from_secs(60 * 60));
        let mut total = 0;
        loop {
            let ids: Vec<(String,)> = {
                let mut conn = self.db.acquire().await?;
                sqlx::query_as(
                    "SELECT id FROM media WHERE (removed_ms IS NOT NULL OR expires_ms < $1) \
                     AND (hold_until_ms IS NULL OR hold_until_ms < $1) LIMIT $2",
                )
                .bind(now_ms)
                .bind(BATCH)
                .fetch_all(&mut *conn)
                .await?
            };
            let n = ids.len() as i64;
            let mut deleted = 0;
            for (id,) in ids {
                if let Err(error) = store.delete(&id) {
                    tracing::error!(%error, "cannot delete a media file; trying again later");
                    continue;
                }
                sqlx::query("DELETE FROM media WHERE id = $1")
                    .bind(&id)
                    .execute(&self.db)
                    .await?;
                deleted += 1;
            }
            total += deleted;
            // A batch that deleted nothing would be selected again for ever (a read-only disk,
            // say): stop, the next sweep tries again.
            if n < BATCH || deleted == 0 {
                return Ok(total);
            }
        }
    }

    /// Deletes the files an account sent that no report holds, when the account is deleted.
    /// Held files stay for their report and go with the sweep.
    pub async fn forget_media(&self, account: u64) -> sqlx::Result<()> {
        let now = crate::util::now_ms();
        let store = self.media_store();
        let ids: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM media WHERE uploader_id = $1 AND (hold_until_ms IS NULL \
             OR hold_until_ms < $2)",
        )
        .bind(i(account))
        .bind(now)
        .fetch_all(&self.db)
        .await?;
        for (id,) in ids {
            if store.delete(&id).is_ok() {
                sqlx::query("DELETE FROM media WHERE id = $1")
                    .bind(&id)
                    .execute(&self.db)
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    /// A minimal MP4: ftyp, then moov with an mvhd of `seconds` and a tkhd of 1280 by 720.
    pub fn mp4(seconds: u32) -> Vec<u8> {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&(seconds * 1000).to_be_bytes());
        let mut tkhd = vec![0u8; 84];
        tkhd[76..80].copy_from_slice(&(1280u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(720u32 << 16).to_be_bytes());
        let trak = boxed(b"trak", &boxed(b"tkhd", &tkhd));
        let mut moov_body = boxed(b"mvhd", &mvhd);
        moov_body.extend(trak);
        let mut out = boxed(b"ftyp", b"isom\0\0\0\0isom");
        out.extend(boxed(b"moov", &moov_body));
        out
    }

    #[test]
    fn a_clip_gives_its_length_and_size() {
        let data = mp4(42);
        let meta = read_mp4(&mut std::io::Cursor::new(data)).unwrap();
        assert_eq!(meta.duration_ms, Some(42_000));
        assert_eq!((meta.width, meta.height), (Some(1280), Some(720)));
    }

    #[test]
    fn a_file_that_is_not_an_mp4_is_refused() {
        let junk = vec![7u8; 200];
        assert!(read_mp4(&mut std::io::Cursor::new(junk)).is_err());
        let mut no_moov = boxed(b"ftyp", b"isom\0\0\0\0isom");
        no_moov.extend(boxed(b"mdat", &[0; 16]));
        assert_eq!(
            read_mp4(&mut std::io::Cursor::new(no_moov)),
            Err(Refused::Broken)
        );
    }

    /// Runs `read_mp4` on its own thread, so that a loop that never ends fails the test instead of
    /// hanging it.
    fn read_within_a_second(data: Vec<u8>) -> Result<Meta, Refused> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_mp4(&mut std::io::Cursor::new(data)));
        });
        rx.recv_timeout(std::time::Duration::from_secs(1))
            .expect("read_mp4 did not return")
    }

    #[test]
    fn a_box_size_that_wraps_the_offset_is_refused() {
        // A second box whose 64-bit size added to its offset wraps to zero: without the check the
        // reader would jump back to the start and read the same boxes for ever.
        let mut data = boxed(b"ftyp", b"isom\0\0\0\0isom");
        let at = data.len() as u64;
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(b"free");
        data.extend_from_slice(&(u64::MAX - at + 1).to_be_bytes());
        data.extend_from_slice(&[0; 16]);
        assert_eq!(read_within_a_second(data), Err(Refused::Broken));
    }

    #[test]
    fn a_file_of_endless_empty_boxes_is_refused() {
        let mut data = boxed(b"ftyp", b"isom\0\0\0\0isom");
        for _ in 0..100_000 {
            data.extend(boxed(b"free", &[]));
        }
        data.extend(boxed(b"moov", &[]));
        assert_eq!(read_within_a_second(data), Err(Refused::Broken));
    }

    #[test]
    fn damaged_clips_never_panic_or_hang() {
        // A small deterministic mutation fuzz (xorshift): flip, cut and extend a valid clip.
        let base = mp4(10);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let mut data = base.clone();
            for _ in 0..(next() % 6) {
                let at = (next() % data.len() as u64) as usize;
                data[at] = match next() % 3 {
                    0 => 0xff,
                    1 => 0,
                    _ => next() as u8,
                };
            }
            match next() % 4 {
                0 => data.truncate((next() % data.len() as u64) as usize),
                1 => data.extend(std::iter::repeat_n(0xff, (next() % 64) as usize)),
                _ => {}
            }
            let _ = read_mp4(&mut std::io::Cursor::new(data));
        }
    }
}
