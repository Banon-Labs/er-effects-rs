//! Error surface for the FLVER reader.

/// Failure while reading a FLVER.
///
/// `Unsupported` also covers the case where the wrapped `fstools_formats` reader
/// *panics* (it `panic!`s on an unknown vertex semantic and `assert!`s on non-zero
/// padding). [`crate::parse_structural`] catches that unwind and converts it here so a
/// non-pristine / non-ER FLVER yields a `Result`, not a process abort.
#[derive(Debug, thiserror::Error)]
pub enum FlverError {
    #[error("FLVER parse: {0}")]
    Parse(#[from] std::io::Error),
    #[error("unsupported FLVER: {0}")]
    Unsupported(String),
}
