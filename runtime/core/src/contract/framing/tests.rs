use super::datagram2::{KIND_REPLY, KIND_REQUEST};
use super::*;
use alloc::vec::Vec;

#[test]
fn datagram_request_round_trips() {
    // `encode_request`'s WireFormat leg is covered by the integration
    // tests; here just the header + params split.
    let mut frame = Vec::new();
    frame.extend_from_slice(&3u16.to_le_bytes());
    frame.extend_from_slice(&42u64.to_le_bytes());
    frame.extend_from_slice(b"args");

    let req = DatagramFraming.decode_request(&frame).unwrap();
    assert_eq!(req.call, RequestCall::Id(3));
    assert_eq!(req.request_id, 42);
    assert_eq!(req.params, b"args");
    assert_eq!(DatagramFraming.decode_request(&[0, 0, 0]), None);
}

#[test]
fn datagram_response_round_trips() {
    let f = DatagramFraming;
    let mut ok = Vec::new();
    f.encode_response_ok(7, b"payload", &mut ok);
    assert_eq!(f.decode_response(&ok), Some((7, Envelope::Ok(b"payload"))));

    let mut err = Vec::new();
    f.encode_response_err(7, 2, b"fields", &mut err);
    assert_eq!(
        f.decode_response(&err),
        Some((
            7,
            Envelope::Err {
                id: 2,
                body: b"fields"
            }
        ))
    );

    let mut fault = Vec::new();
    f.encode_response_fault(7, &mut fault);
    assert_eq!(f.decode_response(&fault), Some((7, Envelope::Fault)));
}
#[test]
fn varints_round_trip_in_their_shortest_form() {
    for (value, bytes) in [
        (0u16, &[0x00][..]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (300, &[0xac, 0x02]),
        (u16::MAX, &[0xff, 0xff, 0x03]),
    ] {
        let mut out = Vec::new();
        put_varint(value, &mut out);
        assert_eq!(out, bytes, "{value}");
        assert_eq!(take_varint(&out), Some((value, &[][..])));
    }
    assert_eq!(take_varint(&[0x80, 0x00]), None, "overlong");
    assert_eq!(take_varint(&[0xff, 0xff, 0x04]), None, "past u16");
    assert_eq!(take_varint(&[0x80]), None, "truncated");
}

#[test]
fn a_reply_round_trips() {
    let f = Datagram2Framing;
    for envelope in [
        Envelope::Ok(b"payload"),
        Envelope::Err {
            id: 2,
            body: b"fields",
        },
        Envelope::Fault,
    ] {
        let mut frame = Vec::new();
        f.encode_reply(200, 7, &envelope, &mut frame);
        assert_eq!(frame[0], KIND_REPLY);
        assert_eq!(
            f.decode(&frame),
            Some(Frame::Reply {
                protocol: Some(200),
                request_id: 7,
                envelope
            })
        );
    }
}

#[test]
fn a_request_decodes() {
    let mut frame = Vec::from([KIND_REQUEST, 3]);
    frame.extend_from_slice(&5u16.to_le_bytes());
    frame.extend_from_slice(&42u64.to_le_bytes());
    frame.extend_from_slice(b"args");
    assert_eq!(
        Datagram2Framing.decode(&frame),
        Some(Frame::Request {
            protocol: Some(3),
            request: Request {
                call: RequestCall::Id(5),
                request_id: 42,
                params: b"args"
            },
            one_way: false,
        })
    );
}

#[test]
fn a_datagram3_request_round_trips_by_numbers() {
    let f = Datagram3Framing;
    let mut frame = Vec::new();
    f.encode_request_body(300, 129, "ping", 42, false, b"args", &mut frame);
    assert_eq!(&frame[..5], &[KIND_REQUEST, 0xac, 0x02, 0x81, 0x01]);
    assert_eq!(
        f.decode(&frame),
        Some(Frame::Request {
            protocol: Some(300),
            request: Request {
                call: RequestCall::Id(129),
                request_id: 42,
                params: b"args"
            },
            one_way: false,
        })
    );
    assert_eq!(
        f.decode(&[KIND_REQUEST, 0, 0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 0]),
        None,
        "overlong function"
    );
    let mut reply = Vec::new();
    f.encode_reply(1, 7, &Envelope::Fault, &mut reply);
    let mut two = Vec::new();
    Datagram2Framing.encode_reply(1, 7, &Envelope::Fault, &mut two);
    assert_eq!(reply, two, "replies are datagram@2's");
}

#[test]
fn malformed_frames_are_rejected() {
    let f = Datagram2Framing;
    assert_eq!(f.decode(&[]), None);
    assert_eq!(f.decode(&[2, 0]), None, "unknown kind");
    assert_eq!(f.decode(&[KIND_REQUEST, 0, 1, 0]), None, "short request");
    assert_eq!(
        f.decode(&[KIND_REPLY, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        None,
        "no envelope"
    );
}
