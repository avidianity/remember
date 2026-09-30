use std::fmt;

/// Every failure a caller can see.
///
/// `Display` renders a single agent-facing line without the `error: ` prefix;
/// transports add the prefix so the core stays format-agnostic.
#[derive(Debug)]
pub enum Error {
    /// The request was understood but cannot be honored; the message tells the Agent what to do instead.
    Rejected(String),
    /// The DB was migrated by a newer binary than this one.
    SchemaTooNew {
        found: i64,
        supported: i64,
    },
    Db(rusqlite::Error),
    Io(std::io::Error),
    Encode(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn rejected(message: impl Into<String>) -> Self {
        Error::Rejected(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Rejected(message) => f.write_str(message),
            Error::SchemaTooNew { found, supported } => write!(
                f,
                "remember DB schema v{found} is newer than this binary (v{supported}); restart the agent"
            ),
            Error::Db(e) => write!(f, "database: {e}"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Encode(e) => write!(f, "encode: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
