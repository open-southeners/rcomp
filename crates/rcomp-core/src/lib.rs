//! `rcomp-core` — unified compression and archive library.
//!
//! Provides format detection, a two-layer format model (codec + container),
//! compression levels, stream codecs, progress reporting, and cancellation
//! support.  The `rcomp` CLI and a future Tauri desktop app are its primary
//! consumers.

#![forbid(unsafe_code)]

pub mod codec;
pub mod detect;
pub mod error;
pub mod format;
pub mod level;
pub mod progress;

pub use codec::{Encoder, new_decoder, new_encoder};
pub use detect::{detect, detect_from_bytes, detect_from_extension};
pub use error::{Error, Result};
pub use format::{Codec, Container, Format};
pub use level::Level;
pub use progress::{CancelToken, Entry, Progress, Report};
