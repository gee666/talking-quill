use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use talking_quill_owner_protocol::fake_transport::fake_ordered_transport_pair_with_capacity;
use talking_quill_owner_protocol::schema::{
    AcquireState, Empty, LeaseAcquireResult, Purpose, Request, Response, SuccessResult,
};
use talking_quill_owner_protocol::{
    Bytes32, FakeAuthenticatedMaterial, GatewayMessage, OrderedTransport, ReceiveResult,
    StreamOrderedTransport, TransportError, TransportProgress, U64String, encode_outer_frame,
};

struct Link {
    bytes: [VecDeque<u8>; 2],
    open: [bool; 2],
    write_blocked: [bool; 2],
    flush_blocked: [bool; 2],
    max_chunk: usize,
}

#[derive(Clone)]
struct StreamControl {
    link: Arc<Mutex<Link>>,
    side: usize,
}

impl StreamControl {
    fn set_write_blocked(&self, blocked: bool) {
        self.link.lock().unwrap().write_blocked[self.side] = blocked;
    }

    fn set_flush_blocked(&self, blocked: bool) {
        self.link.lock().unwrap().flush_blocked[self.side] = blocked;
    }
}

struct TestStream {
    link: Arc<Mutex<Link>>,
    side: usize,
}

impl Drop for TestStream {
    fn drop(&mut self) {
        self.link.lock().unwrap().open[self.side] = false;
    }
}

impl Read for TestStream {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let mut link = self.link.lock().unwrap();
        let peer = 1 - self.side;
        if link.bytes[peer].is_empty() {
            return if link.open[peer] {
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            } else {
                Ok(0)
            };
        }
        let count = output.len().min(link.max_chunk).min(link.bytes[peer].len());
        for byte in &mut output[..count] {
            *byte = link.bytes[peer].pop_front().unwrap();
        }
        Ok(count)
    }
}

impl Write for TestStream {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let mut link = self.link.lock().unwrap();
        if link.write_blocked[self.side] {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        if !link.open[1 - self.side] {
            return Ok(0);
        }
        let count = input.len().min(link.max_chunk);
        link.bytes[self.side].extend(&input[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.link.lock().unwrap().flush_blocked[self.side] {
            Err(io::Error::from(io::ErrorKind::WouldBlock))
        } else {
            Ok(())
        }
    }
}

fn stream_pair(max_chunk: usize) -> ((TestStream, StreamControl), (TestStream, StreamControl)) {
    let link = Arc::new(Mutex::new(Link {
        bytes: [VecDeque::new(), VecDeque::new()],
        open: [true, true],
        write_blocked: [false, false],
        flush_blocked: [false, false],
        max_chunk,
    }));
    (
        (
            TestStream {
                link: Arc::clone(&link),
                side: 0,
            },
            StreamControl {
                link: Arc::clone(&link),
                side: 0,
            },
        ),
        (
            TestStream {
                link: Arc::clone(&link),
                side: 1,
            },
            StreamControl { link, side: 1 },
        ),
    )
}

#[test]
fn normal_transports_reject_malformed_outer_frames_before_queue_mutation() {
    let malformed = [
        Vec::new(),
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 2, 1],
        vec![0, 0, 0, 1, 1, 2],
    ];
    let (mut fake, _) =
        fake_ordered_transport_pair_with_capacity(NonZeroUsize::new(4).unwrap()).unwrap();
    let ((stream, _), _) = stream_pair(8);
    let mut real =
        StreamOrderedTransport::with_capacity(stream, NonZeroUsize::new(4).unwrap()).unwrap();
    for frame in malformed {
        assert_eq!(
            fake.try_send(frame.clone()),
            Err(TransportError::InvalidFrame)
        );
        assert_eq!(real.try_send(frame), Err(TransportError::InvalidFrame));
    }
    assert_eq!(fake.queued_outbound(), 0);
    assert_eq!(real.queued_outbound(), 0);
}

#[test]
fn fake_close_reports_peer_loss_instead_of_delivering_pending_bytes() {
    let (mut writer, mut peer) =
        fake_ordered_transport_pair_with_capacity(NonZeroUsize::new(1).unwrap()).unwrap();
    writer
        .try_send(encode_outer_frame(b"pending").unwrap())
        .unwrap();
    peer.abort();
    assert_eq!(
        OrderedTransport::close(&mut writer),
        Err(TransportError::PeerClosed)
    );
    assert_eq!(writer.queued_outbound(), 1);
}

#[test]
fn stream_transport_preserves_frame_order_across_partial_io() {
    let ((left, _), (right, _)) = stream_pair(3);
    let mut writer = StreamOrderedTransport::new(left).unwrap();
    let mut reader = StreamOrderedTransport::new(right).unwrap();
    let frames = [
        encode_outer_frame(b"first").unwrap(),
        encode_outer_frame(b"second").unwrap(),
    ];
    for frame in &frames {
        let receipt = writer.try_send(frame.clone()).unwrap();
        assert_eq!(writer.flush(receipt).unwrap(), TransportProgress::Complete);
    }
    for expected in frames {
        assert_eq!(
            reader.try_receive().unwrap(),
            ReceiveResult::Frame(expected)
        );
    }
}

#[test]
fn stream_backpressure_is_bounded_and_flush_is_retryable() {
    let ((left, control), (_right, _)) = stream_pair(2);
    let mut writer =
        StreamOrderedTransport::with_capacity(left, NonZeroUsize::new(1).unwrap()).unwrap();
    control.set_write_blocked(true);
    let receipt = writer
        .try_send(encode_outer_frame(b"pending").unwrap())
        .unwrap();
    assert_eq!(writer.flush(receipt).unwrap(), TransportProgress::Pending);
    assert_eq!(writer.queued_outbound(), 1);
    assert_eq!(
        writer.try_send(encode_outer_frame(b"rejected").unwrap()),
        Err(TransportError::QueueFull)
    );
    control.set_write_blocked(false);
    assert_eq!(writer.flush(receipt).unwrap(), TransportProgress::Complete);
    assert_eq!(writer.queued_outbound(), 0);
}

#[test]
fn receipt_and_close_remain_retryable_when_underlying_flush_would_block() {
    let ((left, control), (_right, _)) = stream_pair(64);
    let mut writer = StreamOrderedTransport::new(left).unwrap();
    control.set_flush_blocked(true);
    let receipt = writer
        .try_send(encode_outer_frame(b"written-not-flushed").unwrap())
        .unwrap();
    assert_eq!(writer.flush(receipt).unwrap(), TransportProgress::Pending);
    assert_eq!(writer.close().unwrap(), TransportProgress::Pending);
    control.set_flush_blocked(false);
    assert_eq!(writer.flush(receipt).unwrap(), TransportProgress::Complete);
    assert_eq!(writer.close().unwrap(), TransportProgress::Complete);
}

#[test]
fn graceful_close_flushes_before_peer_eof_and_truncation_is_distinct() {
    let ((left, _), (right, _)) = stream_pair(1);
    let mut writer = StreamOrderedTransport::new(left).unwrap();
    let mut reader = StreamOrderedTransport::new(right).unwrap();
    let frame = encode_outer_frame(b"last").unwrap();
    writer.try_send(frame.clone()).unwrap();
    assert_eq!(writer.close().unwrap(), TransportProgress::Complete);
    assert_eq!(reader.try_receive().unwrap(), ReceiveResult::Frame(frame));
    assert_eq!(reader.try_receive().unwrap(), ReceiveResult::PeerClosed);

    let ((mut raw, _), (right, _)) = stream_pair(8);
    raw.write_all(&[0, 0]).unwrap();
    drop(raw);
    let mut reader = StreamOrderedTransport::new(right).unwrap();
    assert_eq!(reader.try_receive(), Err(TransportError::TruncatedFrame));
}

#[test]
fn authenticated_codecs_have_identical_ordering_over_fragmented_streams() {
    let material = FakeAuthenticatedMaterial::new(
        Bytes32::new([61; 32]),
        Purpose::Capture,
        [62; 32],
        [63; 32],
    );
    let (mut gateway_codec, mut owner_codec) = material.codecs().unwrap();
    let ((left, _), (right, _)) = stream_pair(1);
    let mut gateway_transport = StreamOrderedTransport::new(left).unwrap();
    let mut owner_transport = StreamOrderedTransport::new(right).unwrap();

    let request = gateway_codec
        .encode_request(&Request::LeaseAcquire(Empty {}))
        .unwrap();
    let request_sequence = request.transport_sequence();
    let receipt = gateway_transport.try_send(request.into_frame()).unwrap();
    assert_eq!(
        gateway_transport.flush(receipt).unwrap(),
        TransportProgress::Complete
    );
    let ReceiveResult::Frame(frame) = owner_transport.try_receive().unwrap() else {
        panic!("authenticated request frame")
    };
    let request = owner_codec.receive_request(&frame).unwrap();
    let response = Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
        capture_lease_id: Bytes32::new([64; 32]),
        capture_lease_epoch: U64String::new(std::num::NonZeroU64::new(1).unwrap()),
        state: AcquireState::Disabled,
    }));
    let frame = owner_codec.encode_response(&request, &response).unwrap();
    let receipt = owner_transport.try_send(frame).unwrap();
    assert_eq!(
        owner_transport.flush(receipt).unwrap(),
        TransportProgress::Complete
    );
    let ReceiveResult::Frame(frame) = gateway_transport.try_receive().unwrap() else {
        panic!("authenticated response frame")
    };
    assert!(matches!(
        gateway_codec.receive_owner_frame(&frame).unwrap(),
        GatewayMessage::Response {
            correlation_sequence,
            response: actual,
        } if correlation_sequence == request_sequence && actual == response
    ));
}

#[test]
fn fake_and_stream_transports_deliver_identical_outer_frames() {
    let frames = [
        encode_outer_frame(b"one").unwrap(),
        encode_outer_frame(&vec![7; 16 * 1024]).unwrap(),
    ];
    let (mut fake_writer, mut fake_reader) =
        fake_ordered_transport_pair_with_capacity(NonZeroUsize::new(2).unwrap()).unwrap();
    let ((left, _), (right, _)) = stream_pair(5);
    let mut real_writer =
        StreamOrderedTransport::with_capacity(left, NonZeroUsize::new(2).unwrap()).unwrap();
    let mut real_reader = StreamOrderedTransport::new(right).unwrap();
    for frame in &frames {
        let receipt = fake_writer.try_send(frame.clone()).unwrap();
        fake_writer.confirm_flushed(receipt).unwrap();
        let receipt = real_writer.try_send(frame.clone()).unwrap();
        assert_eq!(
            real_writer.flush(receipt).unwrap(),
            TransportProgress::Complete
        );
    }
    for expected in frames {
        assert_eq!(
            fake_reader.try_receive(),
            ReceiveResult::Frame(expected.clone())
        );
        assert_eq!(
            real_reader.try_receive().unwrap(),
            ReceiveResult::Frame(expected)
        );
    }
}

#[test]
fn bounded_frame_fragmentation_property_holds_for_deterministic_corpus() {
    let lengths = [1, 2, 3, 4, 31, 255, 1024, 4095, 16 * 1024];
    for (case, length) in lengths.into_iter().enumerate() {
        let body = (0..length)
            .map(|index| ((index * 31 + case * 17) & 0xff) as u8)
            .collect::<Vec<_>>();
        for chunk in 1..32 {
            let ((left, _), (right, _)) = stream_pair(chunk);
            let mut writer = StreamOrderedTransport::new(left).unwrap();
            let mut reader = StreamOrderedTransport::new(right).unwrap();
            let frame = encode_outer_frame(&body).unwrap();
            let receipt = writer.try_send(frame.clone()).unwrap();
            assert_eq!(writer.flush(receipt).unwrap(), TransportProgress::Complete);
            assert_eq!(reader.try_receive().unwrap(), ReceiveResult::Frame(frame));
        }
    }
}
