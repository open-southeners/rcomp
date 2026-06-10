//! SHA-256 hashing helpers used by the compress and extract operations.
//!
//! Two thin adapters are provided:
//!
//! - [`HashingWriter`] — wraps a [`Write`] sink and feeds every written byte
//!   through a [`Sha256`] hasher.  Used on the **compress** path to hash the
//!   output file (artifact digest) or an intermediate tar stream (content
//!   digest).
//!
//! - [`HashingReader`] — wraps a [`Read`] source and feeds every byte read
//!   through a [`Sha256`] hasher.  Used on the **compress** path for the
//!   codec-only-file content digest, and on the **extract** path for the
//!   content digest during decompression.
//!
//! Both adapters hold the hasher state in an [`Arc<Mutex<Sha256>>`] so the
//! caller can finalize the digest after the stream is fully consumed even if
//! the adapter has been moved (e.g. boxed into a `dyn Read`).
//!
//! # Producing a hex digest
//!
//! ```ignore
//! use rcomp_core::hash::hex_digest;
//! use sha2::Sha256;
//! use sha2::digest::Digest;
//!
//! let mut hasher = Sha256::new();
//! hasher.update(b"hello");
//! let hex = hex_digest(hasher);
//! ```

use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};

use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// HashingWriter
// ---------------------------------------------------------------------------

/// A [`Write`] adapter that tees every byte through a [`Sha256`] hasher.
///
/// Construct one with a shared [`Arc<Mutex<Sha256>>`] state and wrap it around
/// any writer.  After the write session is complete, finalize the hasher via
/// the shared state.
///
/// # Example
///
/// ```ignore
/// use std::sync::{Arc, Mutex};
/// use rcomp_core::hash::{HashingWriter, hex_digest};
/// use sha2::Sha256;
///
/// let hasher = Arc::new(Mutex::new(Sha256::new()));
/// let mut data = Vec::new();
/// let mut w = HashingWriter::new(&mut data, Arc::clone(&hasher));
/// std::io::Write::write_all(&mut w, b"hello").unwrap();
/// drop(w);
/// let hex = hex_digest(Arc::try_unwrap(hasher).unwrap().into_inner().unwrap());
/// ```
pub(crate) struct HashingWriter<W: Write> {
    inner: W,
    hasher: Arc<Mutex<Sha256>>,
}

impl<W: Write> HashingWriter<W> {
    /// Create a new `HashingWriter` that writes to `inner` and updates `hasher`
    /// on every [`Write::write`] call.
    pub(crate) fn new(inner: W, hasher: Arc<Mutex<Sha256>>) -> Self {
        Self { inner, hasher }
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        if n > 0 {
            self.hasher
                .lock()
                .expect("hash mutex poisoned")
                .update(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// ---------------------------------------------------------------------------
// HashingReader
// ---------------------------------------------------------------------------

/// A [`Read`] adapter that tees every byte through a [`Sha256`] hasher.
///
/// The hasher state is held in an [`Arc<Mutex<Sha256>>`] so the caller can
/// finalize the digest after the stream is fully consumed even if the adapter
/// has been boxed.
///
/// # Example
///
/// ```ignore
/// use std::sync::{Arc, Mutex};
/// use rcomp_core::hash::{HashingReader, hex_digest};
/// use sha2::Sha256;
///
/// let src: &[u8] = b"hello";
/// let hasher = Arc::new(Mutex::new(Sha256::new()));
/// let mut r = HashingReader::new(src, Arc::clone(&hasher));
/// let mut buf = Vec::new();
/// std::io::Read::read_to_end(&mut r, &mut buf).unwrap();
/// let hex = hex_digest(Arc::try_unwrap(hasher).unwrap().into_inner().unwrap());
/// ```
pub(crate) struct HashingReader<R: Read> {
    inner: R,
    hasher: Arc<Mutex<Sha256>>,
}

impl<R: Read> HashingReader<R> {
    /// Create a new `HashingReader` that reads from `inner` and updates `hasher`
    /// on every [`Read::read`] call.
    pub(crate) fn new(inner: R, hasher: Arc<Mutex<Sha256>>) -> Self {
        Self { inner, hasher }
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher
                .lock()
                .expect("hash mutex poisoned")
                .update(&buf[..n]);
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Finalize a [`Sha256`] instance and return the lowercase hex digest.
pub(crate) fn hex_digest(hasher: Sha256) -> String {
    let result = hasher.finalize();
    result.iter().map(|b| format!("{b:02x}")).collect()
}

/// Finalize an [`Arc<Mutex<Sha256>>`] by taking the lock and returning the
/// lowercase hex digest.
///
/// # Panics
///
/// Panics if the mutex is poisoned (which can only happen if another thread
/// panicked while holding the lock — not a normal scenario).
pub(crate) fn finalize_shared(hasher: Arc<Mutex<Sha256>>) -> String {
    let guard = hasher.lock().expect("hash mutex poisoned");
    // Clone the hasher so we can finalize without consuming it — the Arc
    // may still have other owners at finalization time in some code paths.
    hex_digest(guard.clone())
}
