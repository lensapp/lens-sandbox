use std::io::{self, Read};

use serde::{Serialize, de::DeserializeOwned};
use tokio::io::AsyncReadExt;

pub const MAX_FRAME_SIZE: u32 = 1024 * 1024;

pub const FRAME_MAGIC: [u8; 4] = *b"LNS2";

pub const SUBTYPE_JSON: u8 = 0x01;
pub const SUBTYPE_STDOUT: u8 = 0x02;
pub const SUBTYPE_STDERR: u8 = 0x03;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("frame too large: {size} bytes (max {MAX_FRAME_SIZE})")]
    TooLarge { size: u32 },

    #[error("frame length must be at least 1 (sub-type byte)")]
    Empty,

    #[error("bad frame magic: expected {:?}, got {got:?}", FRAME_MAGIC)]
    BadMagic { got: [u8; 4] },

    #[error("unexpected sub-type: expected JSON (0x01), got 0x{got:02x}")]
    UnexpectedSubType { got: u8 },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn encode_raw_frame(sub_type: u8, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    let len: u32 = (1 + payload.len())
        .try_into()
        .map_err(|_| FrameError::TooLarge { size: u32::MAX })?;
    if len > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge { size: len });
    }
    let mut buf = Vec::with_capacity(4 + 4 + len as usize);
    buf.extend_from_slice(&FRAME_MAGIC);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.push(sub_type);
    buf.extend_from_slice(payload);
    Ok(buf)
}

pub fn decode_raw_frame<R: Read>(reader: &mut R) -> Result<(u8, Vec<u8>), FrameError> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != FRAME_MAGIC {
        return Err(FrameError::BadMagic { got: magic });
    }
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge { size: len });
    }
    if len == 0 {
        return Err(FrameError::Empty);
    }
    let mut body = vec![0u8; len as usize];
    reader.read_exact(&mut body)?;
    let sub_type = body[0];
    let payload = body[1..].to_vec();
    Ok((sub_type, payload))
}

pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let json = serde_json::to_vec(msg)?;
    encode_raw_frame(SUBTYPE_JSON, &json)
}

pub fn decode_frame<T: DeserializeOwned, R: Read>(reader: &mut R) -> Result<T, FrameError> {
    let (sub_type, payload) = decode_raw_frame(reader)?;
    if sub_type != SUBTYPE_JSON {
        return Err(FrameError::UnexpectedSubType { got: sub_type });
    }
    let msg = serde_json::from_slice(&payload)?;
    Ok(msg)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireFrame {
    Json(crate::protocol::Response),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

pub fn encode_wire_frame(frame: &WireFrame) -> Result<Vec<u8>, FrameError> {
    match frame {
        WireFrame::Json(resp) => {
            let json = serde_json::to_vec(resp)?;
            encode_raw_frame(SUBTYPE_JSON, &json)
        }
        WireFrame::Stdout(bytes) => encode_raw_frame(SUBTYPE_STDOUT, bytes),
        WireFrame::Stderr(bytes) => encode_raw_frame(SUBTYPE_STDERR, bytes),
    }
}

pub fn decode_wire_frame_from_payload(
    sub_type: u8,
    payload: Vec<u8>,
) -> Result<WireFrame, FrameError> {
    match sub_type {
        SUBTYPE_JSON => {
            let resp = serde_json::from_slice(&payload)?;
            Ok(WireFrame::Json(resp))
        }
        SUBTYPE_STDOUT => Ok(WireFrame::Stdout(payload)),
        SUBTYPE_STDERR => Ok(WireFrame::Stderr(payload)),
        other => Err(FrameError::UnexpectedSubType { got: other }),
    }
}

pub fn decode_wire_frame_sync<R: Read>(reader: &mut R) -> Result<WireFrame, FrameError> {
    let (sub_type, payload) = decode_raw_frame(reader)?;
    decode_wire_frame_from_payload(sub_type, payload)
}

pub fn decode_wire_frame_from_bytes(buf: &[u8]) -> Result<WireFrame, FrameError> {
    let (sub_type, payload) = decode_raw_frame(&mut &buf[..])?;
    decode_wire_frame_from_payload(sub_type, payload)
}

pub async fn read_frame_bytes_async<R>(reader: &mut R) -> Result<Vec<u8>, FrameError>
where
    R: AsyncReadExt + Unpin,
{
    let mut header = [0u8; 8];
    reader.read_exact(&mut header).await?;
    let magic: [u8; 4] = header[..4].try_into().expect("4-byte slice");
    if magic != FRAME_MAGIC {
        return Err(FrameError::BadMagic { got: magic });
    }
    let len = u32::from_be_bytes(header[4..8].try_into().expect("4-byte slice"));
    if len > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge { size: len });
    }
    let mut buf = vec![0u8; 8 + len as usize];
    buf[..8].copy_from_slice(&header);
    reader.read_exact(&mut buf[8..]).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Request, Response, StatusInfo};

    #[test]
    fn round_trip_ping() {
        let req = Request::Ping;
        let frame = encode_frame(&req).unwrap();
        let decoded: Request = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn round_trip_status_request() {
        let req = Request::Status;
        let frame = encode_frame(&req).unwrap();
        let decoded: Request = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn round_trip_shutdown() {
        let req = Request::Shutdown;
        let frame = encode_frame(&req).unwrap();
        let decoded: Request = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn round_trip_unknown() {
        let req = Request::Unknown {
            method: "future_thing".into(),
        };
        let frame = encode_frame(&req).unwrap();
        let decoded: Request = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn round_trip_pong() {
        let resp = Response::Pong;
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn round_trip_status_response() {
        let resp = Response::Status(StatusInfo {
            pid: 12345,
            uptime_secs: 3600,
            version: "0.0.0".into(),
        });
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn round_trip_shutting_down() {
        let resp = Response::ShuttingDown;
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn round_trip_cancel_accepted() {
        let resp = Response::CancelAccepted;
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn round_trip_acknowledged() {
        let resp = Response::Acknowledged;
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn round_trip_error_response() {
        let resp = Response::Error {
            message: "something went wrong".into(),
        };
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn round_trip_run_list_response() {
        let resp = Response::RunList {
            runs: vec![
                crate::RunSummary {
                    id: 7,
                    image: "alpine:latest".into(),
                    command: "sleep 30".into(),
                    status: crate::RunStatus::Running,
                    started: "2026-05-19T12:34:01Z".into(),
                },
                crate::RunSummary {
                    id: 8,
                    image: "<imageless>".into(),
                    command: String::new(),
                    status: crate::RunStatus::Exited { code: 143 },
                    started: "2026-05-19T12:35:01Z".into(),
                },
            ],
        };
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn round_trip_run_list_empty() {
        let resp = Response::RunList { runs: vec![] };
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn rejects_frame_too_large() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&FRAME_MAGIC);
        let oversized = MAX_FRAME_SIZE + 1;
        buf.extend_from_slice(&oversized.to_be_bytes());
        buf.extend(vec![0u8; oversized as usize]);

        let result: Result<Request, _> = decode_frame(&mut &buf[..]);
        assert!(matches!(result, Err(FrameError::TooLarge { .. })));
    }

    #[test]
    fn encode_rejects_oversized_payload() {
        let huge = "x".repeat(MAX_FRAME_SIZE as usize + 16);
        let req = Request::Unknown { method: huge };
        let result = encode_frame(&req);
        let err = result.expect_err("oversized payload must bail");
        assert!(
            matches!(err, FrameError::TooLarge { size } if size > MAX_FRAME_SIZE),
            "expected TooLarge with size > MAX_FRAME_SIZE, got {err:?}"
        );
    }

    #[test]
    fn rejects_zero_length_frame() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&FRAME_MAGIC);
        buf.extend_from_slice(&0u32.to_be_bytes());

        let result: Result<Request, _> = decode_frame(&mut &buf[..]);
        let err = result.expect_err("len=0 must bail");
        let msg = format!("{err:#}");
        assert!(
            matches!(err, FrameError::Empty),
            "expected Empty, got {err:?}"
        );
        assert!(msg.contains("at least 1"), "unexpected message: {msg}");
    }

    #[test]
    fn truncated_frame_returns_io_error() {
        let req = Request::Ping;
        let frame = encode_frame(&req).unwrap();
        let truncated = &frame[..frame.len() - 2];

        let result: Result<Request, _> = decode_frame(&mut &truncated[..]);
        assert!(matches!(result, Err(FrameError::Io(_))));
    }

    #[test]
    fn truncated_header_returns_io_error() {
        let buf = [0u8; 2];
        let result: Result<Request, _> = decode_frame(&mut &buf[..]);
        assert!(matches!(result, Err(FrameError::Io(_))));
    }

    #[test]
    fn rejects_unknown_magic() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"XXXX");
        buf.extend_from_slice(&4u32.to_be_bytes());
        buf.extend_from_slice(b"junk");
        let result: Result<Request, _> = decode_frame(&mut &buf[..]);
        assert!(matches!(result, Err(FrameError::BadMagic { got }) if &got == b"XXXX"));
    }

    #[test]
    fn encoded_frame_starts_with_magic() {
        let frame = encode_frame(&Request::Ping).unwrap();
        assert_eq!(&frame[..4], &FRAME_MAGIC);
    }

    #[test]
    fn round_trip_raw_stdout_subtype() {
        let payload = b"hello";
        let frame = encode_raw_frame(SUBTYPE_STDOUT, payload).unwrap();
        let (sub_type, decoded_payload) = decode_raw_frame(&mut &frame[..]).unwrap();
        assert_eq!(sub_type, SUBTYPE_STDOUT);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn round_trip_raw_stderr_subtype() {
        let payload = b"error output";
        let frame = encode_raw_frame(SUBTYPE_STDERR, payload).unwrap();
        let (sub_type, decoded_payload) = decode_raw_frame(&mut &frame[..]).unwrap();
        assert_eq!(sub_type, SUBTYPE_STDERR);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn decode_frame_rejects_non_json_subtype() {
        let frame = encode_raw_frame(SUBTYPE_STDOUT, b"raw bytes").unwrap();
        let result: Result<Response, _> = decode_frame(&mut &frame[..]);
        assert!(matches!(
            result,
            Err(FrameError::UnexpectedSubType { got: 0x02 })
        ));
    }

    #[test]
    fn bad_magic_v1_rejected() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"LNS1");
        buf.extend_from_slice(&4u32.to_be_bytes());
        buf.extend_from_slice(b"junk");
        let result: Result<Request, _> = decode_frame(&mut &buf[..]);
        assert!(matches!(result, Err(FrameError::BadMagic { got }) if &got == b"LNS1"));
    }

    #[test]
    fn raw_frame_empty_payload_round_trips() {
        let frame = encode_raw_frame(SUBTYPE_STDOUT, b"").unwrap();
        let (sub_type, payload) = decode_raw_frame(&mut &frame[..]).unwrap();
        assert_eq!(sub_type, SUBTYPE_STDOUT);
        assert!(payload.is_empty());
    }

    #[tokio::test]
    async fn read_frame_async_round_trips_through_decode() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let frame = encode_frame(&Request::Ping).unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            a.write_all(&frame).await.unwrap();
        });
        let bytes = read_frame_bytes_async(&mut b).await.unwrap();
        let decoded: Request = decode_frame(&mut &bytes[..]).unwrap();
        assert_eq!(decoded, Request::Ping);
    }

    #[tokio::test]
    async fn read_frame_async_handles_split_writes() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let frame = encode_frame(&Request::Ping).unwrap();
        let split = 2;
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            a.write_all(&frame[..split]).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            a.write_all(&frame[split..]).await.unwrap();
        });
        let bytes = read_frame_bytes_async(&mut b).await.unwrap();
        let decoded: Request = decode_frame(&mut &bytes[..]).unwrap();
        assert_eq!(decoded, Request::Ping);
    }

    #[tokio::test]
    async fn read_frame_async_rejects_oversized_prefix() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let too_big: u32 = MAX_FRAME_SIZE + 1;
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            a.write_all(&FRAME_MAGIC).await.unwrap();
            a.write_all(&too_big.to_be_bytes()).await.unwrap();
        });
        let err = read_frame_bytes_async(&mut b)
            .await
            .expect_err("oversized prefix must be rejected");
        assert!(matches!(err, FrameError::TooLarge { .. }));
    }

    #[tokio::test]
    async fn read_frame_async_rejects_bad_magic() {
        let (mut a, mut b) = tokio::io::duplex(64);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            a.write_all(b"XXXX").await.unwrap();
            a.write_all(&4u32.to_be_bytes()).await.unwrap();
            a.write_all(b"junk").await.unwrap();
        });
        let err = read_frame_bytes_async(&mut b)
            .await
            .expect_err("bad magic must be rejected");
        assert!(matches!(err, FrameError::BadMagic { got } if &got == b"XXXX"));
    }

    #[tokio::test]
    async fn read_frame_async_raw_frame_round_trips_through_decode_raw() {
        let (mut a, mut b) = tokio::io::duplex(128);
        let frame = encode_raw_frame(SUBTYPE_STDERR, b"async stderr").unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            a.write_all(&frame).await.unwrap();
        });
        let bytes = read_frame_bytes_async(&mut b).await.unwrap();
        let (sub_type, payload) = decode_raw_frame(&mut &bytes[..]).unwrap();
        assert_eq!(sub_type, SUBTYPE_STDERR);
        assert_eq!(payload, b"async stderr");
    }

    #[test]
    fn wire_frame_round_trip_json() {
        let frame = WireFrame::Json(Response::Pong);
        let encoded = encode_wire_frame(&frame).unwrap();
        let decoded = decode_wire_frame_sync(&mut &encoded[..]).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn wire_frame_round_trip_stdout() {
        let payload = b"hello world".to_vec();
        let frame = WireFrame::Stdout(payload.clone());
        let encoded = encode_wire_frame(&frame).unwrap();
        let decoded = decode_wire_frame_sync(&mut &encoded[..]).unwrap();
        assert_eq!(decoded, frame);
        assert!(encoded.windows(payload.len()).any(|w| w == payload));
    }

    #[test]
    fn wire_frame_round_trip_stderr() {
        let payload = b"error bytes".to_vec();
        let frame = WireFrame::Stderr(payload.clone());
        let encoded = encode_wire_frame(&frame).unwrap();
        let decoded = decode_wire_frame_sync(&mut &encoded[..]).unwrap();
        assert_eq!(decoded, frame);
        assert!(encoded.windows(payload.len()).any(|w| w == payload));
    }

    #[test]
    fn wire_frame_decode_from_bytes_splits_subtype() {
        let frame = encode_raw_frame(SUBTYPE_STDOUT, b"hi").unwrap();
        let decoded = decode_wire_frame_from_bytes(&frame).unwrap();
        assert_eq!(decoded, WireFrame::Stdout(b"hi".to_vec()));
    }

    #[test]
    fn wire_frame_unknown_subtype_errors() {
        let result = decode_wire_frame_from_payload(0x99, vec![]);
        assert!(matches!(
            result,
            Err(FrameError::UnexpectedSubType { got: 0x99 })
        ));
    }

    #[test]
    fn max_frame_size_is_one_mib() {
        assert_eq!(MAX_FRAME_SIZE, 1024 * 1024);
        assert_eq!(MAX_FRAME_SIZE, 1_048_576);
    }

    #[test]
    fn encode_accepts_frame_at_exactly_max_size() {
        let payload = vec![0u8; MAX_FRAME_SIZE as usize - 1];
        let result = encode_raw_frame(SUBTYPE_STDOUT, &payload);
        assert!(
            result.is_ok(),
            "frame at exactly MAX_FRAME_SIZE must succeed"
        );
    }

    #[test]
    fn encode_rejects_frame_one_byte_over_max() {
        let payload = vec![0u8; MAX_FRAME_SIZE as usize];
        let result = encode_raw_frame(SUBTYPE_STDOUT, &payload);
        assert!(matches!(result, Err(FrameError::TooLarge { .. })));
    }

    #[test]
    fn decode_accepts_frame_at_exactly_max_size() {
        let payload = vec![0u8; MAX_FRAME_SIZE as usize - 1];
        let frame = encode_raw_frame(SUBTYPE_STDOUT, &payload).unwrap();
        let (sub_type, decoded) = decode_raw_frame(&mut &frame[..]).unwrap();
        assert_eq!(sub_type, SUBTYPE_STDOUT);
        assert_eq!(decoded.len(), payload.len());
    }

    #[test]
    fn decode_rejects_frame_one_byte_over_max() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&FRAME_MAGIC);
        let oversized = MAX_FRAME_SIZE + 1;
        buf.extend_from_slice(&oversized.to_be_bytes());
        buf.extend(vec![0u8; oversized as usize]);
        let result = decode_raw_frame(&mut &buf[..]);
        assert!(matches!(result, Err(FrameError::TooLarge { size }) if size == oversized));
    }

    #[tokio::test]
    async fn read_frame_async_accepts_frame_at_exactly_max_size() {
        let payload = vec![0u8; MAX_FRAME_SIZE as usize - 1];
        let frame = encode_raw_frame(SUBTYPE_STDOUT, &payload).unwrap();
        let (mut a, mut b) = tokio::io::duplex(frame.len() + 64);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            a.write_all(&frame).await.unwrap();
        });
        let bytes = read_frame_bytes_async(&mut b).await.unwrap();
        let (sub_type, decoded) = decode_raw_frame(&mut &bytes[..]).unwrap();
        assert_eq!(sub_type, SUBTYPE_STDOUT);
        assert_eq!(decoded.len(), payload.len());
    }

    #[tokio::test]
    async fn read_frame_async_rejects_frame_one_byte_over_max() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let oversized = MAX_FRAME_SIZE + 1;
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            a.write_all(&FRAME_MAGIC).await.unwrap();
            a.write_all(&oversized.to_be_bytes()).await.unwrap();
        });
        let err = read_frame_bytes_async(&mut b)
            .await
            .expect_err("one over max must be rejected");
        assert!(matches!(err, FrameError::TooLarge { size } if size == oversized));
    }
}
