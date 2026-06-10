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

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }
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
