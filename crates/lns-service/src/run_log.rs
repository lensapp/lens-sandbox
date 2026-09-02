use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use lns_ipc::{Response, WireFrame};
use tokio::sync::{mpsc, watch};

pub const DEFAULT_CAPACITY_BYTES: usize = 2 * 1024 * 1024;

pub const ABORTED_EXIT_CODE: i32 = 130;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub seq: u64,
    pub kind: StreamKind,
    pub bytes: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReadBatch {
    pub chunks: Vec<Chunk>,
    pub next_seq: u64,
    pub exit: Option<i32>,
}

#[derive(Debug)]
pub struct RunLogBuffer {
    state: Mutex<State>,
    version: watch::Sender<u64>,
    capacity: usize,
}

#[derive(Debug, Default)]
struct State {
    chunks: VecDeque<Chunk>,
    next_seq: u64,
    bytes: usize,
    exit: Option<i32>,
}

impl Default for RunLogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY_BYTES)
    }
}

impl RunLogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            version: watch::Sender::new(0),
            capacity,
        }
    }

    pub fn append(&self, kind: StreamKind, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut s = self.state.lock().expect("run-log state poisoned");
            let seq = s.next_seq;
            s.next_seq += 1;
            s.bytes += bytes.len();
            s.chunks.push_back(Chunk {
                seq,
                kind,
                bytes: bytes.to_vec(),
            });
            while s.bytes > self.capacity && s.chunks.len() > 1 {
                let evicted = s.chunks.pop_front().expect("len checked above");
                s.bytes -= evicted.bytes.len();
            }
        }
        self.version.send_modify(|v| *v += 1);
    }

    pub fn close(&self, code: i32) {
        {
            let mut s = self.state.lock().expect("run-log state poisoned");
            if s.exit.is_some() {
                return;
            }
            s.exit = Some(code);
        }
        self.version.send_modify(|v| *v += 1);
    }

    pub fn read_from(&self, seq: u64) -> ReadBatch {
        let s = self.state.lock().expect("run-log state poisoned");
        ReadBatch {
            chunks: s.chunks.iter().filter(|c| c.seq >= seq).cloned().collect(),
            next_seq: s.next_seq,
            exit: s.exit,
        }
    }

    pub fn tail_seq(&self) -> u64 {
        self.state.lock().expect("run-log state poisoned").next_seq
    }

    pub fn exit(&self) -> Option<i32> {
        self.state.lock().expect("run-log state poisoned").exit
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }
}

pub fn log_path(cache_root: &std::path::Path, run_id: &str) -> std::path::PathBuf {
    crate::cache::run_dir(cache_root, run_id).join("logs.wire")
}

/// The capture is stored as the wire frames it was captured from, so the stdout/stderr split needs no second encoding.
pub fn snapshot(buffer: &RunLogBuffer) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in buffer.read_from(0).chunks {
        let frame = match chunk.kind {
            StreamKind::Stdout => WireFrame::Stdout(chunk.bytes),
            StreamKind::Stderr => WireFrame::Stderr(chunk.bytes),
        };
        match lns_ipc::encode_wire_frame(&frame) {
            Ok(bytes) => out.extend_from_slice(&bytes),
            Err(e) => crate::log::warn!("run log chunk not saved: {e}"),
        }
    }
    out
}

/// Sequence numbers restart and the exit is left to the run record, so a restored log is what a fresh reader would see.
pub fn hydrate(bytes: &[u8], capacity: usize) -> RunLogBuffer {
    let buffer = RunLogBuffer::new(capacity);
    let mut cursor = bytes;
    while !cursor.is_empty() {
        match lns_ipc::decode_wire_frame_sync(&mut cursor) {
            Ok(frame) => record_frame(&buffer, &frame),
            Err(e) => {
                crate::log::warn!("run log truncated at a frame that does not decode: {e}");
                break;
            }
        }
    }
    buffer
}

pub async fn save_with<F: crate::image_store::Fs>(
    fs: &F,
    cache_root: &std::path::Path,
    run_id: &str,
    buffer: &RunLogBuffer,
) -> std::io::Result<()> {
    fs.write(&log_path(cache_root, run_id), &snapshot(buffer))
        .await
}

/// A log that cannot be read costs the user their output, never their run, so every failure restores an empty buffer.
pub async fn load_with<F: crate::image_store::Fs>(
    fs: &F,
    cache_root: &std::path::Path,
    run_id: &str,
    capacity: usize,
) -> RunLogBuffer {
    match fs.read(&log_path(cache_root, run_id)).await {
        Ok(bytes) => hydrate(&bytes, capacity),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RunLogBuffer::new(capacity),
        Err(e) => {
            crate::log::warn!("run log for {run_id} not read: {e}");
            RunLogBuffer::new(capacity)
        }
    }
}

/// The tee task appends on its own schedule, so a snapshot taken before the buffer closes would drop the last lines the workload printed.
pub async fn await_close(buffer: &RunLogBuffer, within: std::time::Duration) -> bool {
    let mut version = buffer.subscribe();
    let closed = async {
        loop {
            if buffer.exit().is_some() {
                return;
            }
            version
                .changed()
                .await
                .expect("version sender lives inside the borrowed buffer");
        }
    };
    tokio::time::timeout(within, closed).await.is_ok()
}

/// Which buffer answers for a run: a live one has its own, a listed-but-exited one reads what its last boot wrote down, and an unlisted one has none.
pub async fn buffer_for<F: crate::image_store::Fs>(
    live: Option<Arc<RunLogBuffer>>,
    status: Option<lns_ipc::RunStatus>,
    fs: &F,
    cache_root: &std::path::Path,
    run_id: &str,
) -> Option<Arc<RunLogBuffer>> {
    if let Some(buffer) = live {
        return Some(buffer);
    }
    match status {
        Some(lns_ipc::RunStatus::Exited { code }) => Some(Arc::new(
            stopped_buffer_with(fs, cache_root, run_id, code).await,
        )),
        _ => None,
    }
}

/// A stopped run's output lives on disk and its exit code in its record, so a reader gets the same two things a live run's buffer holds.
pub async fn stopped_buffer_with<F: crate::image_store::Fs>(
    fs: &F,
    cache_root: &std::path::Path,
    run_id: &str,
    exit_code: i32,
) -> RunLogBuffer {
    let buffer = load_with(fs, cache_root, run_id, DEFAULT_CAPACITY_BYTES).await;
    buffer.close(exit_code);
    buffer
}

pub async fn tee_frames(
    mut frames: mpsc::Receiver<WireFrame>,
    buffer: Arc<RunLogBuffer>,
    pump_tx: mpsc::Sender<WireFrame>,
) {
    let mut forward = true;
    while let Some(frame) = frames.recv().await {
        record_frame(&buffer, &frame);
        if forward && pump_tx.send(frame).await.is_err() {
            forward = false;
        }
    }
    buffer.close(ABORTED_EXIT_CODE);
}

fn record_frame(buffer: &RunLogBuffer, frame: &WireFrame) {
    match frame {
        WireFrame::Stdout(b) => buffer.append(StreamKind::Stdout, b),
        WireFrame::Stderr(b) => buffer.append(StreamKind::Stderr, b),
        WireFrame::Json(Response::RunExit { code }) => buffer.close(*code),
        WireFrame::Json(_) => {}
    }
}

pub async fn stream_to<W>(
    buffer: &RunLogBuffer,
    writer: &mut W,
    follow: bool,
    mut cursor: u64,
) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let mut version = buffer.subscribe();
    loop {
        version.mark_unchanged();
        let batch = buffer.read_from(cursor);
        cursor = batch.next_seq;
        for chunk in &batch.chunks {
            let wire = match chunk.kind {
                StreamKind::Stdout => WireFrame::Stdout(chunk.bytes.clone()),
                StreamKind::Stderr => WireFrame::Stderr(chunk.bytes.clone()),
            };
            writer
                .write_all(&lns_ipc::encode_wire_frame(&wire)?)
                .await?;
        }
        if let Some(code) = batch.exit {
            let exit = WireFrame::Json(Response::RunExit { code });
            writer
                .write_all(&lns_ipc::encode_wire_frame(&exit)?)
                .await?;
            return Ok(());
        }
        if !follow {
            let end = WireFrame::Json(Response::Acknowledged);
            writer.write_all(&lns_ipc::encode_wire_frame(&end)?).await?;
            return Ok(());
        }
        version
            .changed()
            .await
            .expect("version sender lives inside the borrowed buffer");
    }
}

#[cfg(test)]
mod durability_tests {
    use super::*;

    fn filled(chunks: &[(StreamKind, &[u8])]) -> RunLogBuffer {
        let buffer = RunLogBuffer::default();
        for (kind, bytes) in chunks {
            buffer.append(*kind, bytes);
        }
        buffer
    }

    #[test]
    fn a_saved_log_comes_back_with_both_streams_in_order() {
        let buffer = filled(&[
            (StreamKind::Stdout, b"first"),
            (StreamKind::Stderr, b"warning"),
            (StreamKind::Stdout, b"second"),
        ]);
        let restored = hydrate(&snapshot(&buffer), DEFAULT_CAPACITY_BYTES);
        let batch = restored.read_from(0);
        assert_eq!(
            batch
                .chunks
                .iter()
                .map(|c| (c.kind, c.bytes.clone()))
                .collect::<Vec<_>>(),
            vec![
                (StreamKind::Stdout, b"first".to_vec()),
                (StreamKind::Stderr, b"warning".to_vec()),
                (StreamKind::Stdout, b"second".to_vec()),
            ],
            "a reader cannot tell stdout from stderr unless the split survives the disk"
        );
    }

    #[test]
    fn a_restored_log_is_read_from_the_beginning_again() {
        let buffer = filled(&[(StreamKind::Stdout, b"a"), (StreamKind::Stdout, b"b")]);
        let restored = hydrate(&snapshot(&buffer), DEFAULT_CAPACITY_BYTES);
        assert_eq!(
            restored.read_from(0).chunks.len(),
            2,
            "sequence numbers restart, so a fresh reader still sees everything"
        );
    }

    #[test]
    fn a_restored_log_keeps_only_what_fits_the_cap() {
        let buffer = filled(&[
            (StreamKind::Stdout, b"oldest"),
            (StreamKind::Stdout, b"newest"),
        ]);
        let restored = hydrate(&snapshot(&buffer), 6);
        let chunks = restored.read_from(0).chunks;
        assert_eq!(
            chunks.iter().map(|c| c.bytes.clone()).collect::<Vec<_>>(),
            vec![b"newest".to_vec()],
            "the cap is what the service keeps, so it binds a log that comes back from disk too"
        );
    }

    #[test]
    fn an_exit_is_not_stored_because_the_record_already_knows_it() {
        let buffer = filled(&[(StreamKind::Stdout, b"out")]);
        buffer.close(3);
        assert_eq!(
            hydrate(&snapshot(&buffer), DEFAULT_CAPACITY_BYTES).exit(),
            None
        );
    }

    #[test]
    fn a_damaged_log_file_yields_what_it_can_rather_than_failing() {
        let buffer = filled(&[(StreamKind::Stdout, b"kept")]);
        let mut bytes = snapshot(&buffer);
        bytes.extend_from_slice(b"\xff\xff torn tail");
        let restored = hydrate(&bytes, DEFAULT_CAPACITY_BYTES);
        assert_eq!(
            restored.read_from(0).chunks.len(),
            1,
            "a torn log must not cost the user the part that is intact"
        );
    }

    struct StoredFs(Option<Vec<u8>>);

    impl crate::image_store::Fs for StoredFs {
        async fn read_dir(&self, _: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn read(&self, _: &std::path::Path) -> std::io::Result<Vec<u8>> {
            self.0
                .clone()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn write(&self, _: &std::path::Path, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        async fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_stopped_runs_logs_come_back_with_the_exit_code_its_record_kept() {
        let buffer = filled(&[(StreamKind::Stdout, b"done")]);
        let stored = StoredFs(Some(snapshot(&buffer)));
        let restored =
            stopped_buffer_with(&stored, std::path::Path::new("/cache"), "aa07", 3).await;
        let batch = restored.read_from(0);
        assert_eq!(batch.chunks[0].bytes, b"done".to_vec());
        assert_eq!(
            batch.exit,
            Some(3),
            "a reader must not hang waiting for a run that already ended"
        );
    }

    struct DeniedFs;

    impl crate::image_store::Fs for DeniedFs {
        async fn read_dir(&self, _: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn read(&self, _: &std::path::Path) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        }
        async fn write(&self, _: &std::path::Path, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        async fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_save_waits_for_the_tee_to_finish_appending() {
        let buffer = Arc::new(RunLogBuffer::default());
        buffer.append(StreamKind::Stdout, b"early");
        let late = buffer.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            late.append(StreamKind::Stdout, b"last line before exit");
            late.close(0);
        });
        assert!(await_close(&buffer, std::time::Duration::from_secs(5)).await);
        let saved = snapshot(&buffer);
        let restored = hydrate(&saved, DEFAULT_CAPACITY_BYTES);
        assert_eq!(
            restored
                .read_from(0)
                .chunks
                .iter()
                .map(|c| c.bytes.clone())
                .collect::<Vec<_>>(),
            vec![b"early".to_vec(), b"last line before exit".to_vec()],
            "the lines just before exit are the ones a reader opens `lns logs` for"
        );
    }

    #[tokio::test]
    async fn a_tee_that_never_closes_the_buffer_does_not_wedge_the_runs_end() {
        let buffer = RunLogBuffer::default();
        buffer.append(StreamKind::Stdout, b"partial");
        assert!(
            !await_close(&buffer, std::time::Duration::from_millis(20)).await,
            "a stalled tee must cost the tail, never the run's end"
        );
    }

    #[tokio::test]
    async fn a_live_run_answers_from_its_own_buffer() {
        let live = Arc::new(RunLogBuffer::default());
        live.append(StreamKind::Stdout, b"live");
        let got = buffer_for(
            Some(live),
            Some(lns_ipc::RunStatus::Running),
            &StoredFs(None),
            std::path::Path::new("/cache"),
            "aa07",
        )
        .await
        .expect("a live run has a buffer");
        assert_eq!(got.read_from(0).chunks[0].bytes, b"live".to_vec());
    }

    #[tokio::test]
    async fn a_stopped_run_answers_from_what_its_last_boot_wrote_down() {
        let previous = filled(&[(StreamKind::Stdout, b"from disk")]);
        let got = buffer_for(
            None,
            Some(lns_ipc::RunStatus::Exited { code: 2 }),
            &StoredFs(Some(snapshot(&previous))),
            std::path::Path::new("/cache"),
            "aa07",
        )
        .await
        .expect("a listed stopped run answers");
        let batch = got.read_from(0);
        assert_eq!(batch.chunks[0].bytes, b"from disk".to_vec());
        assert_eq!(batch.exit, Some(2));
    }

    #[tokio::test]
    async fn a_run_nothing_lists_has_no_logs_to_answer_with() {
        for status in [None, Some(lns_ipc::RunStatus::Running)] {
            assert!(
                buffer_for(
                    None,
                    status,
                    &StoredFs(None),
                    std::path::Path::new("/cache"),
                    "aa07"
                )
                .await
                .is_none()
            );
        }
    }

    #[tokio::test]
    async fn a_log_that_cannot_be_read_costs_the_output_and_not_the_run() {
        let restored =
            stopped_buffer_with(&DeniedFs, std::path::Path::new("/cache"), "aa07", 0).await;
        assert!(restored.read_from(0).chunks.is_empty());
        assert_eq!(
            restored.read_from(0).exit,
            Some(0),
            "an unreadable log must still let a reader learn the run ended"
        );
    }

    #[test]
    fn a_chunk_too_large_to_frame_is_dropped_rather_than_costing_the_whole_log() {
        let buffer = RunLogBuffer::new(8 * 1024 * 1024);
        buffer.append(StreamKind::Stdout, b"kept");
        buffer.append(
            StreamKind::Stdout,
            &vec![b'x'; lns_ipc::MAX_FRAME_SIZE as usize + 16],
        );
        let restored = hydrate(&snapshot(&buffer), DEFAULT_CAPACITY_BYTES);
        assert_eq!(
            restored
                .read_from(0)
                .chunks
                .iter()
                .map(|c| c.bytes.clone())
                .collect::<Vec<_>>(),
            vec![b"kept".to_vec()]
        );
    }

    #[tokio::test]
    async fn the_test_fakes_honour_their_whole_port_surface() {
        use crate::image_store::Fs as _;
        let stored = StoredFs(None);
        assert!(stored.read_dir(std::path::Path::new("/")).await.is_err());
        assert!(stored.write(std::path::Path::new("/x"), b"").await.is_ok());
        assert!(stored.remove_file(std::path::Path::new("/x")).await.is_ok());
        assert!(DeniedFs.read_dir(std::path::Path::new("/")).await.is_err());
        assert!(
            DeniedFs
                .write(std::path::Path::new("/x"), b"")
                .await
                .is_ok()
        );
        assert!(
            DeniedFs
                .remove_file(std::path::Path::new("/x"))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_stopped_run_with_no_saved_log_still_reports_its_exit() {
        let restored =
            stopped_buffer_with(&StoredFs(None), std::path::Path::new("/cache"), "aa07", 0).await;
        let batch = restored.read_from(0);
        assert!(batch.chunks.is_empty());
        assert_eq!(batch.exit, Some(0));
    }

    #[test]
    fn an_empty_log_restores_to_an_empty_buffer() {
        assert!(
            hydrate(&[], DEFAULT_CAPACITY_BYTES)
                .read_from(0)
                .chunks
                .is_empty()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_increasing_seqs_and_read_from_zero_returns_all_in_order() {
        let buf = RunLogBuffer::new(1024);
        buf.append(StreamKind::Stdout, b"one");
        buf.append(StreamKind::Stderr, b"two");
        let batch = buf.read_from(0);
        assert_eq!(batch.next_seq, 2);
        assert_eq!(batch.exit, None);
        assert_eq!(
            batch.chunks,
            vec![
                Chunk {
                    seq: 0,
                    kind: StreamKind::Stdout,
                    bytes: b"one".to_vec()
                },
                Chunk {
                    seq: 1,
                    kind: StreamKind::Stderr,
                    bytes: b"two".to_vec()
                },
            ]
        );
    }

    #[test]
    fn read_from_cursor_skips_chunks_already_seen() {
        let buf = RunLogBuffer::new(1024);
        buf.append(StreamKind::Stdout, b"old");
        let cursor = buf.tail_seq();
        buf.append(StreamKind::Stdout, b"new");
        let batch = buf.read_from(cursor);
        assert_eq!(batch.chunks.len(), 1);
        assert_eq!(batch.chunks[0].bytes, b"new");
    }

    #[test]
    fn empty_append_consumes_no_seq() {
        let buf = RunLogBuffer::new(1024);
        buf.append(StreamKind::Stdout, b"");
        assert_eq!(buf.tail_seq(), 0);
        assert!(buf.read_from(0).chunks.is_empty());
    }

    #[test]
    fn eviction_drops_oldest_chunks_once_capacity_is_exceeded() {
        let buf = RunLogBuffer::new(8);
        buf.append(StreamKind::Stdout, b"aaaa");
        buf.append(StreamKind::Stdout, b"bbbb");
        buf.append(StreamKind::Stdout, b"cccc");
        let batch = buf.read_from(0);
        assert_eq!(
            batch.chunks.iter().map(|c| c.seq).collect::<Vec<_>>(),
            vec![1, 2],
            "oldest chunk must be evicted, later seqs retained"
        );
        assert_eq!(batch.next_seq, 3);
    }

    #[test]
    fn eviction_keeps_the_newest_chunk_even_when_it_alone_exceeds_capacity() {
        let buf = RunLogBuffer::new(4);
        buf.append(StreamKind::Stdout, b"tiny");
        buf.append(StreamKind::Stdout, b"this-is-way-over-capacity");
        let batch = buf.read_from(0);
        assert_eq!(batch.chunks.len(), 1);
        assert_eq!(batch.chunks[0].bytes, b"this-is-way-over-capacity");
    }

    #[test]
    fn close_records_exit_and_first_close_wins() {
        let buf = RunLogBuffer::new(1024);
        buf.close(7);
        buf.close(99);
        assert_eq!(buf.read_from(0).exit, Some(7));
    }

    #[test]
    fn exit_is_none_until_closed_then_reports_the_code() {
        let buf = RunLogBuffer::new(1024);
        assert_eq!(buf.exit(), None);
        buf.close(42);
        assert_eq!(buf.exit(), Some(42));
    }

    #[tokio::test]
    async fn subscriber_wakes_on_append_and_on_close() {
        let buf = RunLogBuffer::new(1024);
        let mut rx = buf.subscribe();
        rx.mark_unchanged();
        buf.append(StreamKind::Stdout, b"x");
        rx.changed().await.expect("append must bump the version");
        buf.close(0);
        rx.changed().await.expect("close must bump the version");
    }

    #[test]
    fn default_buffer_uses_the_shipping_capacity() {
        let buf = RunLogBuffer::default();
        assert_eq!(buf.capacity, DEFAULT_CAPACITY_BYTES);
    }

    #[tokio::test]
    async fn tee_records_output_and_exit_while_forwarding_every_frame_in_order() {
        let buf = Arc::new(RunLogBuffer::new(1024));
        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);

        in_tx
            .send(WireFrame::Stdout(b"out".to_vec()))
            .await
            .unwrap();
        in_tx
            .send(WireFrame::Stderr(b"err".to_vec()))
            .await
            .unwrap();
        in_tx
            .send(WireFrame::Json(Response::RunLog {
                level: lns_ipc::LogLevel::Info,
                verb: Some("Booted".into()),
                message: "microVM".into(),
            }))
            .await
            .unwrap();
        in_tx
            .send(WireFrame::Json(Response::RunExit { code: 3 }))
            .await
            .unwrap();
        drop(in_tx);

        tee_frames(in_rx, buf.clone(), out_tx).await;

        let batch = buf.read_from(0);
        assert_eq!(
            batch.chunks.len(),
            2,
            "RunLog frames are not workload output"
        );
        assert_eq!(batch.chunks[0].kind, StreamKind::Stdout);
        assert_eq!(batch.chunks[1].kind, StreamKind::Stderr);
        assert_eq!(batch.exit, Some(3));

        let mut forwarded = Vec::new();
        while let Ok(f) = out_rx.try_recv() {
            forwarded.push(f);
        }
        assert_eq!(forwarded.len(), 4, "every frame must reach the pump");
        assert!(matches!(forwarded[0], WireFrame::Stdout(_)));
        assert!(matches!(
            forwarded[3],
            WireFrame::Json(Response::RunExit { code: 3 })
        ));
    }

    #[tokio::test]
    async fn tee_keeps_recording_after_the_pump_receiver_is_gone() {
        let buf = Arc::new(RunLogBuffer::new(1024));
        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);

        in_tx.send(WireFrame::Stdout(b"a".to_vec())).await.unwrap();
        in_tx.send(WireFrame::Stdout(b"b".to_vec())).await.unwrap();
        drop(in_tx);

        tee_frames(in_rx, buf.clone(), out_tx).await;

        assert_eq!(
            buf.read_from(0).chunks.len(),
            2,
            "a detached CLI must not stop log capture"
        );
    }

    #[tokio::test]
    async fn tee_closes_with_aborted_code_when_frames_end_without_an_exit() {
        let buf = Arc::new(RunLogBuffer::new(1024));
        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        in_tx.send(WireFrame::Stdout(b"a".to_vec())).await.unwrap();
        drop(in_tx);

        tee_frames(in_rx, buf.clone(), out_tx).await;

        assert_eq!(buf.read_from(0).exit, Some(ABORTED_EXIT_CODE));
    }

    fn decode_frames(mut buf: &[u8]) -> Vec<WireFrame> {
        let mut out = Vec::new();
        while !buf.is_empty() {
            out.push(lns_ipc::decode_wire_frame_sync(&mut buf).expect("decode wire frame"));
        }
        out
    }

    #[tokio::test]
    async fn stream_without_follow_dumps_buffered_output_and_ends_with_acknowledged() {
        let buf = RunLogBuffer::new(1024);
        buf.append(StreamKind::Stdout, b"hello ");
        buf.append(StreamKind::Stderr, b"oops");

        let mut sink: Vec<u8> = Vec::new();
        stream_to(&buf, &mut sink, false, 0).await.unwrap();

        let frames = decode_frames(&sink);
        assert_eq!(frames.len(), 3);
        assert!(matches!(&frames[0], WireFrame::Stdout(b) if b == b"hello "));
        assert!(matches!(&frames[1], WireFrame::Stderr(b) if b == b"oops"));
        assert!(matches!(frames[2], WireFrame::Json(Response::Acknowledged)));
    }

    #[tokio::test]
    async fn stream_of_an_exited_buffer_ends_with_the_runs_exit_code() {
        let buf = RunLogBuffer::new(1024);
        buf.append(StreamKind::Stdout, b"bye");
        buf.close(7);

        let mut sink: Vec<u8> = Vec::new();
        stream_to(&buf, &mut sink, false, 0).await.unwrap();

        let frames = decode_frames(&sink);
        assert!(matches!(
            frames.last(),
            Some(WireFrame::Json(Response::RunExit { code: 7 }))
        ));
    }

    #[tokio::test]
    async fn follow_waits_for_output_appended_after_it_drained_the_buffer() {
        let buf = Arc::new(RunLogBuffer::new(1024));
        buf.append(StreamKind::Stdout, b"early");

        let (mut client, server) = tokio::io::duplex(8192);
        let streamer = {
            let buf = buf.clone();
            tokio::spawn(async move {
                let mut server = server;
                stream_to(&buf, &mut server, true, 0).await
            })
        };

        let first = lns_ipc::read_frame_bytes_async(&mut client)
            .await
            .expect("the buffered chunk streams immediately");
        assert!(matches!(
            lns_ipc::decode_wire_frame_from_bytes(&first).unwrap(),
            WireFrame::Stdout(b) if b == b"early"
        ));

        buf.append(StreamKind::Stdout, b"late");
        buf.close(0);

        streamer.await.unwrap().unwrap();

        let mut received = Vec::new();
        use tokio::io::AsyncReadExt;
        client.read_to_end(&mut received).await.unwrap();
        let frames = decode_frames(&received);
        assert!(matches!(&frames[0], WireFrame::Stdout(b) if b == b"late"));
        assert!(matches!(
            frames.last(),
            Some(WireFrame::Json(Response::RunExit { code: 0 }))
        ));
    }

    #[tokio::test]
    async fn follow_from_the_tail_skips_history_the_way_attach_expects() {
        let buf = RunLogBuffer::new(1024);
        buf.append(StreamKind::Stdout, b"history");
        let cursor = buf.tail_seq();
        buf.append(StreamKind::Stdout, b"fresh");
        buf.close(3);

        let mut sink: Vec<u8> = Vec::new();
        stream_to(&buf, &mut sink, true, cursor).await.unwrap();

        let frames = decode_frames(&sink);
        assert_eq!(frames.len(), 2, "history must not be replayed");
        assert!(matches!(&frames[0], WireFrame::Stdout(b) if b == b"fresh"));
        assert!(matches!(
            frames[1],
            WireFrame::Json(Response::RunExit { code: 3 })
        ));
    }

    #[tokio::test]
    async fn stream_surfaces_a_write_failure() {
        let buf = RunLogBuffer::new(1024);
        buf.append(StreamKind::Stdout, vec![0u8; 1024].as_slice());

        let (client, mut server) = tokio::io::duplex(64);
        drop(client);
        let err = stream_to(&buf, &mut server, false, 0).await;
        assert!(err.is_err(), "closed peer must surface as an error");
    }

    #[tokio::test]
    async fn tee_does_not_override_a_real_exit_with_the_aborted_code() {
        let buf = Arc::new(RunLogBuffer::new(1024));
        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        in_tx
            .send(WireFrame::Json(Response::RunExit { code: 0 }))
            .await
            .unwrap();
        drop(in_tx);

        tee_frames(in_rx, buf.clone(), out_tx).await;

        assert_eq!(buf.read_from(0).exit, Some(0));
    }
}
