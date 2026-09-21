//! `itsalive` — bring up HDMI and the Linux framebuffer on a MiSTer without
//! Main_MiSTer running.
//!
//! The crate is split so that everything which is pure arithmetic or pure table
//! lookup ([`video`], [`fb`], the table half of [`adv7513`]) is unit-tested on
//! the host, and everything which touches hardware ([`hw`], and the I/O half of
//! [`mailbox`]) sits behind small traits that a recording fake implements.
//!
//! See `docs/ARCHITECTURE.md` for the register-level contract; every constant
//! in this crate is transcribed from Main_MiSTer at commit `6cda9cc` and cites
//! its `file:line`.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod adv7513;
pub mod fb;
pub mod hw;
pub mod mailbox;
pub mod say;
pub mod video;

use std::fmt;

/// Every way this tool is allowed to fail.
///
/// The installer treats any non-zero exit as "no splash this time", so the
/// discriminants matter more than the messages: see `docs/ARCHITECTURE.md` §7.
#[derive(Debug)]
pub enum Error {
    /// The command line did not parse. Exit 2.
    Usage(String),
    /// GPI bit 31 was set: there is no bitstream in the fabric. Exit 10.
    NoBitstream,
    /// The fabric never acked a mailbox strobe within the deadline. Exit 11.
    Timeout,
    /// No `/dev/i2c-N` answered at chip address 0x39. Exit 12.
    NoAdv7513,
    /// More than one `/dev/i2c-N` answered at 0x39, so the bus is ambiguous
    /// and picking the first would silently pick the wrong one. Exit 13.
    AmbiguousBus(Vec<u8>),
    /// `/dev/mem`, `/dev/i2c-*`, sysfs or a tty could not be opened or used.
    /// Exit 14.
    Io {
        /// What we were trying to touch, for the stderr line.
        what: String,
        /// The underlying OS error.
        source: std::io::Error,
    },
}

impl Error {
    /// The process exit code for this error, per `docs/ARCHITECTURE.md` §7.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => 2,
            Error::NoBitstream => 10,
            Error::Timeout => 11,
            Error::NoAdv7513 => 12,
            Error::AmbiguousBus(_) => 13,
            Error::Io { .. } => 14,
        }
    }

    /// Build an [`Error::Io`] naming what was being touched.
    pub fn io(what: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            what: what.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(msg) => write!(f, "usage: {msg}"),
            Error::NoBitstream => {
                write!(
                    f,
                    "no bitstream in the fabric (GPI bit 31): load a core first"
                )
            }
            Error::Timeout => write!(f, "mailbox timeout: the fabric never acked"),
            Error::NoAdv7513 => write!(f, "no ADV7513 found at 0x39 on any i2c bus"),
            Error::AmbiguousBus(buses) => {
                write!(f, "ADV7513 answered on more than one i2c bus: {buses:?}")
            }
            Error::Io { what, source } => write!(f, "{what}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Shorthand for the crate's fallible operations.
pub type Result<T> = std::result::Result<T, Error>;
