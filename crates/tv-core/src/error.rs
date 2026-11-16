use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("i/o error: {0}")]
    IoPlain(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("i/o error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("file not found: {0}")]
    NotFound(String),

    #[error("unrecognised file format: {0}")]
    BadFormat(String),

    /// The file is a Chromium SNSS session file, but stored with an OS-level
    /// encrypted body we cannot read without the browser's own key material.
    #[error("encrypted session file (SNSS v{version}); contents cannot be read without the browser key")]
    EncryptedSessionFile { version: i32 },

    /// The file parses structurally but a record is malformed. We keep going and
    /// collect these; the caller decides whether partial data is acceptable.
    #[error("malformed record at offset {offset}: {detail}")]
    MalformedRecord { offset: u64, detail: String },

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl std::fmt::Display, source: std::io::Error) -> Self {
        Error::Io {
            path: path.to_string(),
            source,
        }
    }
}

/// Reads a file, mapping the error with the path attached (so the UI can say
/// *which* profile file failed rather than a bare "the system cannot find...").
pub fn read_file(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| Error::io(path.display(), e))
}
