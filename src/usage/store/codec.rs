//! Compact storage for each observation's payload and pricing snapshot.
//!
//! Schema 4 stores both JSON documents as zstd frames compressed with a dictionary
//! trained on the database's own rows: the pricing snapshot repeats the whole rate
//! card, and the payload repeats field names and identifiers, so a shared dictionary
//! shrinks a typical pair from about 2 KB to a few hundred bytes. Rows written
//! before a dictionary existed, or by an older release, stay TEXT and read unchanged.
//!
//! A stored BLOB is `[MAGIC][FORMAT][dictionary id, u32 little endian][zstd frame]`.
//! MAGIC can never start JSON text, and dictionaries are immutable once written,
//! so a row always decodes with the dictionary it names.
use anyhow::{Context, Result, anyhow, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params, types::ValueRef};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

const MAGIC: u8 = 0xF5;
const WITH_DICTIONARY: u8 = 1;
const HEADER: usize = 6;
/// Stored documents are small metadata records; anything larger is corrupt.
const MAX_DOCUMENT: usize = 4 << 20;
const LEVEL: i32 = 3;
pub(super) const DICTIONARY_BYTES: usize = 64 << 10;
/// Fewer rows than this would train a dictionary on too little evidence.
pub(super) const MIN_TRAINING_ROWS: i64 = 1_000;
const TRAINING_SAMPLE: i64 = 2_000;
/// Rows written after the active dictionary before a fresh one is trained.
pub(super) const RETRAIN_AFTER_ROWS: i64 = 100_000;

pub(crate) struct Dictionary {
    pub id: i64,
    encoder: zstd::dict::EncoderDictionary<'static>,
    decoder: zstd::dict::DecoderDictionary<'static>,
}

/// Dictionaries by (database identity, dictionary id). Each read opens its own
/// connection, so the cache lives here rather than on a connection.
type Cache = HashMap<(String, i64), Arc<Dictionary>>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
const CACHE_LIMIT: usize = 32;

/// A random identity written once per database, so two databases in one process
/// never share cached dictionaries.
pub(crate) fn database_id(conn: &Connection) -> Result<String> {
    conn.query_row("SELECT value FROM usage_meta WHERE key='database_id'", [], |r| r.get(0))
        .optional()?
        .context("usage database has no identity")
}

pub(super) fn ensure_database_id(conn: &Connection) -> Result<()> {
    let id = hex::encode(uuid::Uuid::new_v4().as_bytes());
    conn.execute("INSERT OR IGNORE INTO usage_meta(key,value) VALUES('database_id',?1)", [id])?;
    Ok(())
}

pub(crate) fn dictionary(conn: &Connection, id: i64) -> Result<Arc<Dictionary>> {
    let database = database_id(conn)?;
    dictionary_for(conn, &database, id)
}

fn dictionary_for(conn: &Connection, database: &str, id: i64) -> Result<Arc<Dictionary>> {
    let key = (database.to_owned(), id);
    if let Some(d) = CACHE.lock().unwrap().get(&key) {
        return Ok(d.clone());
    }
    let bytes: Vec<u8> = conn
        .query_row("SELECT dictionary FROM usage_dictionaries WHERE id=?1", [id], |r| r.get(0))
        .optional()?
        .with_context(|| format!("usage dictionary {id} is missing"))?;
    let d = Arc::new(Dictionary {
        id,
        encoder: zstd::dict::EncoderDictionary::copy(&bytes, LEVEL),
        decoder: zstd::dict::DecoderDictionary::copy(&bytes),
    });
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, d.clone());
    Ok(d)
}

/// The dictionary new rows are compressed with, if one has been trained.
pub(crate) fn active(conn: &Connection) -> Result<Option<Arc<Dictionary>>> {
    let id: Option<i64> = conn
        .query_row("SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key='active_dictionary'", [], |r| r.get(0))
        .optional()?;
    id.map(|id| dictionary(conn, id)).transpose()
}

/// Stores documents for one batch: compressed with the active dictionary when the
/// database has one, as plain text before then. One context serves the whole batch.
pub(crate) struct Documents {
    dictionary: Option<Arc<Dictionary>>,
    cctx: zstd::zstd_safe::CCtx<'static>,
    frame: Vec<u8>,
}

impl Documents {
    pub fn new(conn: &Connection) -> Result<Self> {
        Ok(Self { dictionary: active(conn)?, cctx: zstd::zstd_safe::CCtx::create(), frame: Vec::new() })
    }

    pub fn store(&mut self, text: &str) -> Result<rusqlite::types::Value> {
        let Some(dictionary) = &self.dictionary else {
            return Ok(rusqlite::types::Value::Text(text.to_owned()));
        };
        let id = u32::try_from(dictionary.id).context("usage dictionary id out of range")?;
        // Reuse the worst-case scratch allocation for the batch. Stored blobs
        // only need the actual compressed length, which is usually much smaller.
        self.frame.clear();
        self.frame.reserve(zstd::zstd_safe::compress_bound(text.len()));
        self.cctx
            .compress_using_cdict(&mut self.frame, text.as_bytes(), dictionary.encoder.as_cdict())
            .map_err(|code| anyhow!("compress usage document: {}", zstd::zstd_safe::get_error_name(code)))?;
        let mut out = Vec::with_capacity(HEADER + self.frame.len());
        out.extend_from_slice(&[MAGIC, WITH_DICTIONARY]);
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&self.frame);
        Ok(rusqlite::types::Value::Blob(out))
    }

    pub fn compressing(&self) -> bool {
        self.dictionary.is_some()
    }
}

/// A stored payload or snapshot as JSON text, whichever way it was stored.
pub(crate) fn decode(conn: &Connection, value: ValueRef<'_>) -> Result<String> {
    Decoder::new(conn).decode(value)
}

/// Reuse the decompression context and last dictionary within a read/batch. A
/// 500-row details page otherwise creates 1,000 contexts and repeats the database
/// identity query for every document. The connection binds the cached identity
/// to one database, and all size/format validation is still per document.
pub(crate) struct Decoder<'a> {
    conn: &'a Connection,
    database: Option<String>,
    dictionary: Option<Arc<Dictionary>>,
    dctx: Option<zstd::zstd_safe::DCtx<'static>>,
}

impl<'a> Decoder<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn, database: None, dictionary: None, dctx: None }
    }

    pub fn decode(&mut self, value: ValueRef<'_>) -> Result<String> {
        match value {
            ValueRef::Text(t) => Ok(std::str::from_utf8(t).context("stored usage document is not UTF-8")?.to_owned()),
            ValueRef::Blob(b) => {
                ensure!(b.len() > HEADER && b[0] == MAGIC && b[1] == WITH_DICTIONARY, "unrecognized usage document");
                let id = i64::from(u32::from_le_bytes(b[2..HEADER].try_into().unwrap()));
                if self.dictionary.as_ref().is_none_or(|d| d.id != id) {
                    if self.database.is_none() {
                        self.database = Some(database_id(self.conn)?);
                    }
                    self.dictionary = Some(dictionary_for(self.conn, self.database.as_deref().unwrap(), id)?);
                }
                let dictionary = self.dictionary.as_ref().unwrap();
                let frame = &b[HEADER..];
                let size = zstd::zstd_safe::get_frame_content_size(frame)
                    .map_err(|_| anyhow!("corrupt usage document"))?
                    .context("usage document has no recorded size")?;
                let size =
                    usize::try_from(size).ok().filter(|n| *n <= MAX_DOCUMENT).context("usage document too large")?;
                let mut bytes = Vec::with_capacity(size);
                self.dctx
                    .get_or_insert_with(zstd::zstd_safe::DCtx::create)
                    .decompress_using_ddict(&mut bytes, frame, dictionary.decoder.as_ddict())
                    .map_err(|code| anyhow!("corrupt usage document: {}", zstd::zstd_safe::get_error_name(code)))?;
                ensure!(bytes.len() == size, "corrupt usage document");
                String::from_utf8(bytes).context("stored usage document is not UTF-8")
            }
            _ => bail!("stored usage document has an unexpected type"),
        }
    }
}

/// `usage_document(x)` in SQL: a stored document as text, so a query can still read
/// inside one (`json_extract(usage_document(payload),'$.status')`).
pub(crate) fn register(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "usage_document",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8 | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            // SAFETY: the connection is only used to read dictionaries, within this call.
            let conn = unsafe { ctx.get_connection()? };
            decode(&conn, ctx.get_raw(0)).map_err(|e| rusqlite::Error::UserFunctionError(e.into()))
        },
    )
}

/// Trains a dictionary on rows spread across the newest part of the journal, makes
/// it active and returns its id. None when there are too few rows to learn from.
pub(super) fn train(conn: &Connection) -> Result<Option<i64>> {
    let (count, max_id): (i64, i64) =
        conn.query_row("SELECT COUNT(*),COALESCE(MAX(id),0) FROM usage_observations", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    if count < MIN_TRAINING_ROWS {
        return Ok(None);
    }
    // Evenly spaced ids over the newest window keep the sample current and bounded.
    let low = (max_id - 50 * TRAINING_SAMPLE).max(0);
    let step = ((max_id - low) / TRAINING_SAMPLE).max(1);
    let mut samples = Vec::new();
    let mut decoder = Decoder::new(conn);
    {
        let mut statement = conn.prepare(
            "SELECT payload,snapshot_json FROM usage_observations WHERE id>?1 AND (id-?1)%?2=0 ORDER BY id LIMIT ?3",
        )?;
        let mut rows = statement.query(params![low, step, TRAINING_SAMPLE])?;
        while let Some(row) = rows.next()? {
            samples.push(decoder.decode(row.get_ref(0)?)?.into_bytes());
            samples.push(decoder.decode(row.get_ref(1)?)?.into_bytes());
        }
    }
    // Two documents per sampled row; fewer than 500 rows is too little to learn from.
    if samples.len() < 1_000 {
        return Ok(None);
    }
    let dictionary = zstd::dict::from_samples(&samples, DICTIONARY_BYTES).context("train usage dictionary")?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO usage_dictionaries(created_at_ms,trained_through_id,samples,dictionary) VALUES(?1,?2,?3,?4)",
        params![now, max_id, samples.len() as i64 / 2, dictionary],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO usage_meta(key,value) VALUES('active_dictionary',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [id.to_string()],
    )?;
    Ok(Some(id))
}

/// Whether enough rows arrived since the active dictionary to train a fresh one.
pub(super) fn needs_training(conn: &Connection) -> Result<bool> {
    let trained: Option<i64> = conn
        .query_row(
            "SELECT d.trained_through_id FROM usage_dictionaries d JOIN usage_meta m ON m.key='active_dictionary' AND d.id=CAST(m.value AS INTEGER)",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let (count, max_id): (i64, i64) =
        conn.query_row("SELECT COUNT(*),COALESCE(MAX(id),0) FROM usage_observations", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    Ok(match trained {
        None => count >= MIN_TRAINING_ROWS,
        Some(through) => max_id - through >= RETRAIN_AFTER_ROWS,
    })
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
