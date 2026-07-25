use std::io::{Cursor, Read};

use talking_quill_helper::framing::{FrameError, MAX_FRAME_BYTES, read_frame, write_frame};
use proptest::prelude::*;

struct ChunkedReader {
    inner: Cursor<Vec<u8>>,
    chunk_size: usize,
}

impl Read for ChunkedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let allowed = buffer.len().min(self.chunk_size);
        self.inner.read(&mut buffer[..allowed])
    }
}

fn framed(payload: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    write_frame(&mut output, payload).expect("valid frame");
    output
}

#[test]
fn round_trips_an_exact_16_kib_maximum_sized_frame() {
    assert_eq!(MAX_FRAME_BYTES, 16 * 1024);
    let payload = vec![b'x'; MAX_FRAME_BYTES];
    let mut input = Cursor::new(framed(&payload));
    assert_eq!(read_frame(&mut input).unwrap(), Some(payload));
    assert_eq!(read_frame(&mut input).unwrap(), None);
}

#[test]
fn reads_prefix_and_payload_split_into_single_bytes() {
    let payload = br#"{"jsonrpc":"2.0"}"#;
    let mut input = ChunkedReader {
        inner: Cursor::new(framed(payload)),
        chunk_size: 1,
    };
    assert_eq!(read_frame(&mut input).unwrap(), Some(payload.to_vec()));
}

#[test]
fn distinguishes_clean_eof_from_truncation() {
    assert!(matches!(
        read_frame(&mut Cursor::new(Vec::<u8>::new())),
        Ok(None)
    ));
    assert!(matches!(
        read_frame(&mut Cursor::new(vec![0, 0, 0])),
        Err(FrameError::TruncatedPrefix)
    ));
    assert!(matches!(
        read_frame(&mut Cursor::new(vec![0, 0, 0, 2, b'x'])),
        Err(FrameError::TruncatedPayload)
    ));
}

#[test]
fn rejects_zero_and_oversized_lengths_before_payload_allocation() {
    assert!(matches!(
        read_frame(&mut Cursor::new(0_u32.to_be_bytes())),
        Err(FrameError::InvalidLength(0))
    ));
    let oversized = u32::try_from(MAX_FRAME_BYTES + 1).unwrap();
    assert!(matches!(
        read_frame(&mut Cursor::new(oversized.to_be_bytes())),
        Err(FrameError::InvalidLength(value)) if value == oversized
    ));
}

#[test]
fn writer_rejects_empty_and_oversized_payloads() {
    assert!(matches!(
        write_frame(&mut Vec::new(), &[]),
        Err(FrameError::OutboundTooLarge(0))
    ));
    assert!(matches!(
        write_frame(&mut Vec::new(), &vec![0; MAX_FRAME_BYTES + 1]),
        Err(FrameError::OutboundTooLarge(_))
    ));
}

proptest! {
    #[test]
    fn arbitrary_payloads_survive_arbitrary_chunking(
        payload in proptest::collection::vec(any::<u8>(), 1..4096),
        chunk_size in 1_usize..32,
    ) {
        let mut input = ChunkedReader {
            inner: Cursor::new(framed(&payload)),
            chunk_size,
        };
        prop_assert_eq!(read_frame(&mut input).unwrap(), Some(payload));
    }
}
