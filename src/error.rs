#[derive(thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    RubatoC(#[from] rubato::ResamplerConstructionError),

    #[error(transparent)]
    RubatoR(#[from] rubato::ResampleError),

    #[error(transparent)]
    Opus(#[from] opus::Error),

    #[error(transparent)]
    OggRead(#[from] ogg::OggReadError),

    #[error("unexpected ogg signature {0:?}")]
    OggUnexpectedSignature([u8; 8]),

    #[error("unexpected ogg capture pattern {0:?}")]
    OggUnexpectedCapturePattern([u8; 4]),

    #[error("unexpected len for opus head {0}")]
    OggUnexpectedLenForOpusHead(usize),

    #[error("unsupported ogg version {0}")]
    OggUnsupportedVersion(u8),

    #[error("opus pcm was not found")]
    OpusMissingPcm,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Symphonia(#[from] symphonia::core::errors::Error),

    /// User generated error message, typically created via `bail!`.
    #[error("{0}")]
    Msg(String),

    /// Arbitrary errors wrapping.
    #[error("{0}")]
    Wrapped(Box<dyn std::fmt::Display + Send + Sync>),

    #[error("{context}\n{inner}")]
    Context { inner: Box<Self>, context: Box<dyn std::fmt::Display + Send + Sync> },

    /// Adding path information to an error.
    #[error("path: {path:?} {inner}")]
    WithPath { inner: Box<Self>, path: std::path::PathBuf },

    #[error("{inner}\n{backtrace}")]
    WithBacktrace { inner: Box<Self>, backtrace: Box<std::backtrace::Backtrace> },
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[macro_export]
macro_rules! bail {
    ($msg:literal $(,)?) => {
        return Err($crate::Error::Msg(format!($msg).into()).bt())
    };
    ($err:expr $(,)?) => {
        return Err($crate::Error::Msg(format!($err).into()).bt())
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err($crate::Error::Msg(format!($fmt, $($arg)*).into()).bt())
    };
}

impl Error {
    pub fn wrap(err: impl std::fmt::Display + Send + Sync + 'static) -> Self {
        Self::Wrapped(Box::new(err)).bt()
    }

    pub fn msg(err: impl std::fmt::Display) -> Self {
        Self::Msg(err.to_string()).bt()
    }

    pub fn debug(err: impl std::fmt::Debug) -> Self {
        Self::Msg(format!("{err:?}")).bt()
    }

    pub fn bt(self) -> Self {
        let backtrace = std::backtrace::Backtrace::capture();
        match backtrace.status() {
            std::backtrace::BacktraceStatus::Disabled
            | std::backtrace::BacktraceStatus::Unsupported => self,
            _ => Self::WithBacktrace { inner: Box::new(self), backtrace: Box::new(backtrace) },
        }
    }

    pub fn with_path<P: AsRef<std::path::Path>>(self, p: P) -> Self {
        Self::WithPath { inner: Box::new(self), path: p.as_ref().to_path_buf() }
    }

    pub fn context(self, c: impl std::fmt::Display + Send + Sync + 'static) -> Self {
        Self::Context { inner: Box::new(self), context: Box::new(c) }
    }
}
