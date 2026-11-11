//! Ordered parallel chunk compression shared by the codec backends.
//!
//! [`ParallelWriter`] cuts the bytes written to it into fixed-size chunks,
//! compresses the chunks on a pool of worker threads, and hands the results to
//! a [`Sink`] on the calling thread in input order.  The output writer never
//! leaves the calling thread, so it does not need to be `Send`.
//!
//! Memory stays bounded: at most `2 × threads` chunks are in flight (queued,
//! being compressed, or compressed and waiting for an earlier chunk), and
//! [`Write::write`] blocks until the oldest of them has been emitted.
//!
//! The codec decides what a chunk becomes (an LZ4 block, a raw deflate
//! segment, a whole bzip2 stream, …) and writes any header before creating the
//! writer and any trailer after [`ParallelWriter::finish`].

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Receives compressed chunks, in input order, on the thread that owns the
/// output writer.
///
/// Any `FnMut(&mut dyn Write, R) -> io::Result<()>` closure is a `Sink`.  A
/// codec that needs running state for its trailer (a combined CRC, a total
/// length) implements the trait on a struct instead and gets that struct back
/// from [`ParallelWriter::finish`].
pub(crate) trait Sink<R> {
    /// Write one chunk's result to `out`.
    fn emit(&mut self, out: &mut dyn Write, result: R) -> io::Result<()>;
}

impl<R, F> Sink<R> for F
where
    F: FnMut(&mut dyn Write, R) -> io::Result<()>,
{
    fn emit(&mut self, out: &mut dyn Write, result: R) -> io::Result<()> {
        self(out, result)
    }
}

/// The per-chunk compression function run on worker threads.
type Work<R> = dyn Fn(u64, &[u8], bool) -> io::Result<R> + Send + Sync;

struct Job {
    index: u64,
    data: Vec<u8>,
    last: bool,
}

struct Done<R> {
    index: u64,
    result: io::Result<R>,
    /// The chunk's input buffer, cleared, returned for reuse.
    buffer: Vec<u8>,
}

/// A [`Write`] adapter that compresses fixed-size chunks in parallel and
/// emits the results in order.
///
/// Dropping the writer without calling [`finish`][Self::finish] abandons the
/// stream: queued chunks are discarded and the workers exit after the chunk
/// they are on.
pub(crate) struct ParallelWriter<'a, R, S> {
    out: Box<dyn Write + 'a>,
    sink: S,
    chunk_size: usize,
    /// The chunk being filled.  A full chunk is held here until more input
    /// arrives, so the final chunk can be flagged as such.
    buf: Vec<u8>,
    spare: Vec<Vec<u8>>,
    jobs: Option<Sender<Job>>,
    done: Receiver<Done<R>>,
    /// Results that arrived ahead of an earlier chunk.
    ready: BTreeMap<u64, R>,
    submitted: u64,
    emitted: u64,
    max_in_flight: u64,
}

impl<'a, R, S> ParallelWriter<'a, R, S>
where
    R: Send + 'static,
    S: Sink<R>,
{
    /// Start `threads` workers that turn chunks of `chunk_size` bytes into `R`
    /// with `work`, and emit each `R` through `sink` in input order.
    ///
    /// `work` receives the chunk's zero-based index, its bytes, and whether it
    /// is the final chunk.  Every chunk except the final one is exactly
    /// `chunk_size` bytes; the final chunk holds between 1 and `chunk_size`
    /// bytes, or is empty when nothing at all was written.
    ///
    /// `work` runs on a worker thread.  Chunks are handed to the workers from
    /// one first-in, first-out queue, so a chunk is always picked up after
    /// every earlier chunk has been picked up, and every chunk that is picked
    /// up is passed to `work`.  Codecs may rely on that (LZ4 hashes chunks in
    /// index order on the workers).  Chunks finish in any order.
    ///
    /// # Errors
    ///
    /// Returns an error if no worker thread could be started.
    pub(crate) fn new<F>(
        out: Box<dyn Write + 'a>,
        chunk_size: usize,
        threads: usize,
        work: F,
        sink: S,
    ) -> io::Result<Self>
    where
        F: Fn(u64, &[u8], bool) -> io::Result<R> + Send + Sync + 'static,
    {
        assert!(chunk_size > 0, "chunk_size must be non-zero");
        let threads = threads.max(1);
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done_rx) = mpsc::channel::<Done<R>>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let work = Arc::new(work);

        let mut started = 0usize;
        for _ in 0..threads {
            let job_rx = Arc::clone(&job_rx);
            let done_tx = done_tx.clone();
            let work = Arc::clone(&work);
            let spawned = thread::Builder::new()
                // The CLI's panic hook matches this name; keep them in step.
                .name("rcomp-codec-worker".into())
                .spawn(move || worker(&job_rx, &done_tx, &*work));
            match spawned {
                Ok(_) => started += 1,
                Err(e) if started == 0 => return Err(e),
                // Run with the workers that did start.
                Err(_) => break,
            }
        }

        Ok(Self {
            out,
            sink,
            chunk_size,
            buf: Vec::new(),
            spare: Vec::new(),
            jobs: Some(job_tx),
            done: done_rx,
            ready: BTreeMap::new(),
            submitted: 0,
            emitted: 0,
            // A window of twice the workers keeps them fed while the oldest
            // chunk is still on a slow (efficiency) core.  Narrower windows
            // measured 20 to 40% less memory but up to 50% more wall time at
            // slow levels on a 12+4-core machine.
            max_in_flight: (started as u64) * 2,
        })
    }

    /// Compress and emit everything still buffered, then return the output
    /// writer and the sink so the codec can write its trailer.
    ///
    /// # Errors
    ///
    /// Returns the first error reported by `work`, the sink, or the output
    /// writer.
    pub(crate) fn finish(mut self) -> io::Result<(Box<dyn Write + 'a>, S)> {
        self.submit(true)?;
        // Closing the queue lets the workers exit once it is drained.
        self.jobs = None;
        while self.emitted < self.submitted {
            self.receive()?;
            self.emit_ready()?;
        }
        Ok((self.out, self.sink))
    }

    /// Queue the current buffer as the next chunk and apply backpressure.
    fn submit(&mut self, last: bool) -> io::Result<()> {
        let next = self.spare.pop().unwrap_or_default();
        let data = std::mem::replace(&mut self.buf, next);
        let job = Job {
            index: self.submitted,
            data,
            last,
        };
        self.jobs
            .as_ref()
            .ok_or_else(workers_gone)?
            .send(job)
            .map_err(|_| workers_gone())?;
        self.submitted += 1;

        // The oldest unemitted chunk is always either in `ready` (and emitted
        // by `emit_ready`) or still with a worker, so this cannot wait forever.
        while self.submitted - self.emitted >= self.max_in_flight {
            self.receive()?;
            self.emit_ready()?;
        }
        while let Ok(done) = self.done.try_recv() {
            self.accept(done)?;
        }
        self.emit_ready()
    }

    fn receive(&mut self) -> io::Result<()> {
        let done = self.done.recv().map_err(|_| workers_gone())?;
        self.accept(done)
    }

    fn accept(&mut self, done: Done<R>) -> io::Result<()> {
        let result = done.result?;
        self.spare.push(done.buffer);
        self.ready.insert(done.index, result);
        Ok(())
    }

    fn emit_ready(&mut self) -> io::Result<()> {
        while let Some(result) = self.ready.remove(&self.emitted) {
            self.sink.emit(&mut *self.out, result)?;
            self.emitted += 1;
        }
        Ok(())
    }
}

impl<R, S> Write for ParallelWriter<'_, R, S>
where
    R: Send + 'static,
    S: Sink<R>,
{
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut rest = data;
        while !rest.is_empty() {
            if self.buf.len() == self.chunk_size {
                // More input follows this full chunk, so it is not the last.
                self.submit(false)?;
            }
            let n = (self.chunk_size - self.buf.len()).min(rest.len());
            self.buf.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
        }
        Ok(data.len())
    }

    /// Flushes the output writer.  Input still being buffered or compressed
    /// is not forced out; only [`ParallelWriter::finish`] does that.
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

fn worker<R>(jobs: &Mutex<Receiver<Job>>, done: &Sender<Done<R>>, work: &Work<R>) {
    loop {
        let job = match jobs.lock() {
            Ok(queue) => queue.recv(),
            Err(_) => return,
        };
        let Ok(job) = job else { return };
        let result = catch_unwind(AssertUnwindSafe(|| work(job.index, &job.data, job.last)))
            .unwrap_or_else(|_| Err(io::Error::other("compression worker panicked")));
        let mut buffer = job.data;
        buffer.clear();
        let sent = done.send(Done {
            index: job.index,
            result,
            buffer,
        });
        if sent.is_err() {
            // The writer was dropped; nobody wants the remaining chunks.
            return;
        }
    }
}

fn workers_gone() -> io::Error {
    io::Error::other("compression workers stopped unexpectedly")
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::ParallelWriter;

    /// Shared output buffer so a test can inspect what was written.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run `input` through a writer whose "compression" records each chunk as
    /// `(index, len, last)` and whose sink writes the chunk back unchanged.
    fn run(input: &[u8], chunk: usize, threads: usize) -> (Vec<u8>, Vec<(u64, usize, bool)>) {
        let out = Shared::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_in_sink = Arc::clone(&seen);
        let mut writer = ParallelWriter::new(
            Box::new(out.clone()),
            chunk,
            threads,
            |index, data: &[u8], last| {
                // Finish early chunks last to exercise reordering.
                std::thread::sleep(Duration::from_millis(8u64.saturating_sub(index)));
                Ok((index, data.to_vec(), last))
            },
            move |w: &mut dyn Write, (index, data, last): (u64, Vec<u8>, bool)| {
                seen_in_sink.lock().unwrap().push((index, data.len(), last));
                w.write_all(&data)
            },
        )
        .unwrap();
        // Feed in uneven pieces so chunk boundaries fall inside writes.
        for piece in input.chunks(7) {
            writer.write_all(piece).unwrap();
        }
        drop(writer.finish().unwrap());
        let bytes = out.0.lock().unwrap().clone();
        let seen = seen.lock().unwrap().clone();
        (bytes, seen)
    }

    #[test]
    fn output_is_in_input_order() {
        let input: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let (bytes, seen) = run(&input, 16, 4);
        assert_eq!(bytes, input);
        let indices: Vec<u64> = seen.iter().map(|s| s.0).collect();
        assert_eq!(indices, (0..seen.len() as u64).collect::<Vec<_>>());
    }

    #[test]
    fn empty_input_is_one_empty_final_chunk() {
        let (bytes, seen) = run(&[], 16, 2);
        assert!(bytes.is_empty());
        assert_eq!(seen, vec![(0, 0, true)]);
    }

    #[test]
    fn input_of_exactly_one_chunk_is_a_single_final_chunk() {
        let (bytes, seen) = run(&[7u8; 16], 16, 2);
        assert_eq!(bytes, vec![7u8; 16]);
        assert_eq!(seen, vec![(0, 16, true)]);
    }

    #[test]
    fn only_the_final_chunk_is_short_and_flagged() {
        let (_, seen) = run(&[1u8; 33], 16, 3);
        assert_eq!(seen, vec![(0, 16, false), (1, 16, false), (2, 1, true)]);
        let (_, seen) = run(&[1u8; 32], 16, 3);
        assert_eq!(seen, vec![(0, 16, false), (1, 16, true)]);
    }

    #[test]
    fn single_thread_still_works() {
        let input = vec![9u8; 100];
        let (bytes, _) = run(&input, 8, 1);
        assert_eq!(bytes, input);
    }

    #[test]
    fn worker_error_is_returned() {
        let mut writer = ParallelWriter::new(
            Box::new(std::io::sink()),
            4,
            2,
            |index, _data: &[u8], _last| {
                if index == 1 {
                    Err(std::io::Error::other("boom"))
                } else {
                    Ok(())
                }
            },
            |_w: &mut dyn Write, _r: ()| Ok(()),
        )
        .unwrap();
        let result = writer
            .write_all(&[0u8; 64])
            .and_then(|()| writer.finish().map(|_| ()));
        assert_eq!(result.unwrap_err().to_string(), "boom");
    }

    #[test]
    fn worker_panic_becomes_an_error() {
        let mut writer = ParallelWriter::new(
            Box::new(std::io::sink()),
            4,
            2,
            |_index, _data: &[u8], _last| -> std::io::Result<()> { panic!("worker bug") },
            |_w: &mut dyn Write, _r: ()| Ok(()),
        )
        .unwrap();
        let result = writer
            .write_all(&[0u8; 64])
            .and_then(|()| writer.finish().map(|_| ()));
        assert!(result.is_err());
    }

    #[test]
    fn sink_state_is_returned_by_finish() {
        struct Count(usize);
        impl super::Sink<usize> for Count {
            fn emit(&mut self, _out: &mut dyn Write, len: usize) -> std::io::Result<()> {
                self.0 += len;
                Ok(())
            }
        }
        let mut writer = ParallelWriter::new(
            Box::new(std::io::sink()),
            8,
            2,
            |_index, data: &[u8], _last| Ok(data.len()),
            Count(0),
        )
        .unwrap();
        writer.write_all(&[0u8; 50]).unwrap();
        let (_out, count) = writer.finish().unwrap();
        assert_eq!(count.0, 50);
    }
}
