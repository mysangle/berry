use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    IoError(io::Error),
    TooSmallWindow(u16, u16),
    UnknownWindowSize,
    NotUtf8Input(Vec<u8>),
    ControlCharInText(char),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            IoError(err) => write!(f, "{}", err),
            TooSmallWindow(w, h) => write!(
                f,
                "Screen {}x{} is too small. At least 1x3 is necessary in width x height",
                w, h
            ),
            UnknownWindowSize => write!(f, "Could not detect terminal window size"),
            NotUtf8Input(seq) => {
                write!(f, "Cannot handle non-UTF8 multi-byte input sequence: ")?;
                for byte in seq.iter() {
                    write!(f, "\\x{:x}", byte)?;
                }
                Ok(())
            }
            ControlCharInText(c) => write!(f, "Invalid character for text is included: {:?}", c),
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::IoError(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

