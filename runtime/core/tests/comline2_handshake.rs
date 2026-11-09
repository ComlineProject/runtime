//! `comline@2` over a transport (runtime#15): both exchanges, accept,
//! degrade, and refusals that reach both ends with their reason and side.
#![cfg(feature = "std")]

use std::thread;

use comline_runtime::handshake::comline2::*;
use comline_runtime::contract::{
    Reason, RuntimeError, Side, FORMAT_PACKED, FRAMING_DATAGRAM3,
};
use comline_runtime::handshake::{Facts, Handshake};
use comline_runtime::transport::duplex;

fn hash(b: u8) -> Hash128 {
    Hash128([b; 16])
}

fn hello(version: (u32, u32, u32), functions: &[u16], line: (u32, u32, u32)) -> Hello {
    Hello {
        exchange: Exchange::Offer,
        package_id: hash(0xaa),
        version: PackageVersion::release(version.0, version.1, version.2),
        encodings: vec![
            Encoding { kind: EncodingKind::Format, param_hash: hash(1) },
            Encoding { kind: EncodingKind::Framing, param_hash: hash(2) },
        ],
        protocols: vec![Protocol {
            number: 0,
            line: Release::new(line.0, line.1, line.2),
            shape_hash: hash(functions.len() as u8),
            functions: functions.to_vec(),
        }],
    }
}

type Ends = (Result<(), RuntimeError>, Comline2Handshake, Result<(), RuntimeError>, Comline2Handshake);

fn run(mut client: Comline2Handshake, mut server: Comline2Handshake) -> Ends {
    let (mut a, mut b) = duplex();
    let facts = Facts::new(0, FORMAT_PACKED, FRAMING_DATAGRAM3);
    let server = thread::spawn(move || {
        let facts = Facts::new(0, FORMAT_PACKED, FRAMING_DATAGRAM3);
        let r = server.respond(&mut b, &facts);
        (r, server)
    });
    let c = client.initiate(&mut a, &facts);
    let (s, server) = server.join().unwrap();
    (c, client, s, server)
}

#[test]
fn a_newer_consumer_degrades_in_both_exchanges() {
    for exchange in [Exchange::Offer, Exchange::Symmetric] {
        let params = Params { exchange, ..Params::default() };
        let (c, client, s, server) = run(
            Comline2Handshake::new(params, hello((0, 3, 0), &[0, 1, 2], (0, 2, 0))),
            Comline2Handshake::new(params, hello((0, 2, 1), &[0, 1], (0, 2, 0))),
        );
        assert_eq!((c, s), (Ok(()), Ok(())), "{exchange:?}");
        let n = client.negotiated().unwrap();
        assert_eq!(n.peer_version, PackageVersion::release(0, 2, 1));
        assert_eq!(n.check_call(0, 1), Ok(()));
        assert_eq!(
            n.check_call(0, 2),
            Err(RuntimeError::Unavailable),
            "the function the provider lacks fails locally"
        );
        assert_eq!(n.check_call(7, 0), Err(RuntimeError::Unavailable));
        assert!(server.negotiated().unwrap().check_call(0, 1).is_ok());
    }
}

#[test]
fn refusals_reach_both_ends_with_reason_and_side() {
    for exchange in [Exchange::Offer, Exchange::Symmetric] {
        // The provider refuses a newer consumer.
        let params = Params { exchange, ..Params::default() };
        let strict = Params { newer_consumer: NewerConsumer::Refuse, ..params };
        let (c, client, s, _) = run(
            Comline2Handshake::new(params, hello((0, 3, 0), &[0, 1, 2], (0, 2, 0))),
            Comline2Handshake::new(strict, hello((0, 2, 1), &[0, 1], (0, 2, 0))),
        );
        let refused = Err(RuntimeError::Refused { reason: Reason::NewerConsumer, side: Side::Responder });
        assert_eq!((&c, &s), (&refused, &refused), "{exchange:?}");
        assert_eq!(client.refusal().unwrap().protocol, Some(0));

        // Lines differ: the only protocol is refused, so the connection is.
        let (c, _, s, _) = run(
            Comline2Handshake::new(params, hello((0, 3, 0), &[0], (0, 3, 0))),
            Comline2Handshake::new(params, hello((0, 2, 0), &[0], (0, 2, 0))),
        );
        let side = match exchange {
            Exchange::Offer => Side::Responder,
            Exchange::Symmetric => Side::Initiator,
        };
        assert_eq!(c, Err(RuntimeError::Refused { reason: Reason::LineMismatch, side }), "{exchange:?}");
        assert!(matches!(s, Err(RuntimeError::Refused { reason: Reason::LineMismatch, .. })));
    }
}

#[test]
fn exchanges_that_differ_refuse() {
    let offer = Params::default();
    let symmetric = Params { exchange: Exchange::Symmetric, ..offer };
    for (c, s) in [(offer, symmetric), (symmetric, offer)] {
        let (c, _, s, _) = run(
            Comline2Handshake::new(c, hello((0, 1, 0), &[0], (0, 1, 0))),
            Comline2Handshake::new(s, hello((0, 1, 0), &[0], (0, 1, 0))),
        );
        assert!(matches!(c, Err(RuntimeError::Refused { reason: Reason::ExchangeMismatch, .. })), "{c:?}");
        assert!(matches!(s, Err(RuntimeError::Refused { reason: Reason::ExchangeMismatch, .. })), "{s:?}");
    }
}

#[test]
fn a_comline1_frame_is_another_handshake_version() {
    let mut frame = Vec::new();
    comline_runtime::contract::HandshakeFrame::new(1, FORMAT_PACKED, FRAMING_DATAGRAM3, 0).encode(&mut frame);
    assert_eq!(Message::decode(&frame), Err(Reason::HandshakeVersion));
    assert_eq!(Message::decode(b"nope"), Err(Reason::Malformed));
}
