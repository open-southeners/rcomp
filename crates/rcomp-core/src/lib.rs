//! `rcomp-core` — unified compression and archive library.
//!
//! Provides format detection, a two-layer format model (codec + container),
//! compression levels, stream codecs, archive backends (tar, zip), progress
//! reporting, and cancellation support.  The `rcomp` CLI and a future Tauri
//! desktop app are its primary consumers.
//!
//! # Quick start
//!
//! ```no_run
//! use std::path::Path;
//! use rcomp_core::{CompressOptions, ExtractOptions, compress, extract};
//!
//! // Compress a directory to tar.gz (format inferred from extension).
//! let report = compress(
//!     Path::new("/tmp/my-folder"),
//!     Path::new("/tmp/my-folder.tar.gz"),
//!     &CompressOptions::default(),
//!     |_progress| {},
//! ).unwrap();
//! println!("Wrote {} bytes in {:?}", report.output_bytes, report.duration);
//!
//! // Extract the archive back into a destination directory.
//! let report = extract(
//!     Path::new("/tmp/my-folder.tar.gz"),
//!     Path::new("/tmp/restored"),
//!     &ExtractOptions::default(),
//!     |_progress| {},
//! ).unwrap();
//! println!("Extracted {} entries", report.entries);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod archive;
pub mod codec;
pub mod detect;
pub mod error;
pub mod format;
pub(crate) mod hash;
pub mod level;
mod ops;
pub mod progress;
pub(crate) mod walk;

pub use codec::{Encoder, new_decoder, new_encoder};
pub use detect::{detect, detect_from_bytes, detect_from_extension, split_format_suffix};
pub use error::{Error, Result};
pub use format::{Codec, Container, Format};
pub use level::Level;
pub use ops::{CompressOptions, ExtractOptions, compress, extract, list};
pub use progress::{CancelToken, Entry, Progress, Report};
