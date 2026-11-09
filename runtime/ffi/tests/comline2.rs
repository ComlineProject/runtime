// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline@2` on a connection, through the `extern "C"` functions: both
//! exchanges from one end to the other, refusals reaching both ends, what the
//! readers give and when, calls by wire numbers over it, and the ABI's rules on
//! the new functions. `tests/vectors.rs` proves each step against the corpus.

mod common;

use std::ptr;

use comline_runtime::handshake::comline2::{
    Exchange, Hash128, Hello, Message, PackageVersion, Protocol, Release,
};
use comline_runtime_ffi::*;
use common::*;

/// Facts for a package at `version`, with `protocols` as `(number, line,
/// shape, functions)`.
fn facts(
    exchange: Exchange,
    version: (u32, u32, u32),
    protocols: &[(u16, u32, u8, &[u16])],
) -> Vec<u8> {
    Message::Hello(Hello {
        exchange,
        package_id: Hash128([9; 16]),
        version: PackageVersion::release(version.0, version.1, version.2),
        encodings: Vec::new(),
        protocols: protocols
            .iter()
            .map(|(number, line, shape, functions)| Protocol {
                number: *number,
                line: Release::new(0, *line, 0),
                shape_hash: Hash128([*shape; 16]),
                functions: functions.to_vec(),
            })
            .collect(),
    })
    .to_bytes()
}

const BOTH: &[(u16, u32, u8, &[u16])] = &[(0, 1, 1, &[0, 1, 2]), (300, 2, 2, &[0])];

fn exchange_code(exchange: Exchange) -> u8 {
    exchange as u8
}

/// Both ends, each with its facts and `tweak` applied to its config.
fn ends(
    exchange: Exchange,
    initiator: &[u8],
    responder: &[u8],
    tweak: impl Fn(&mut comline_comline2_config),
) -> (Conn, Conn) {
    let mut a = comline2_config(initiator, COMLINE_SIDE_INITIATOR, exchange_code(exchange));
    let mut b = comline2_config(responder, COMLINE_SIDE_RESPONDER, exchange_code(exchange));
    tweak(&mut a);
    tweak(&mut b);
    (
        Conn::comline2("datagram@3", &a).unwrap(),
        Conn::comline2("datagram@3", &b).unwrap(),
    )
}

/// Run messages between the two ends until neither has anything to send: what
/// each last fed status was.
fn run(a: &mut Conn, b: &mut Conn) -> (comline_status, comline_status) {
    let (mut last_a, mut last_b) = (COMLINE_OK, COMLINE_OK);
    for _ in 0..8 {
        let from_a = a.start(4096);
        if from_a.status == COMLINE_OK && !from_a.bytes.is_empty() {
            last_b = b.feed(&from_a.bytes).status;
        }
        let from_b = b.start(4096);
        if from_b.status == COMLINE_OK && !from_b.bytes.is_empty() {
            last_a = a.feed(&from_b.bytes).status;
        }
    }
    (last_a, last_b)
}

#[test]
fn both_exchanges_connect_and_carry_calls_by_number() {
    for exchange in [Exchange::Offer, Exchange::Symmetric] {
        let hello = facts(exchange, (0, 3, 0), BOTH);
        let (mut a, mut b) = ends(exchange, &hello, &hello, |_| {});
        assert_eq!(
            a.request(0, 0, b"", true, b"", 256).status,
            COMLINE_ERR_STATE,
            "not open yet"
        );
        assert_eq!(
            run(&mut a, &mut b),
            (COMLINE_OK, COMLINE_OK),
            "{exchange:?}"
        );

        // Two-byte numbers, each way, by datagram@3.
        assert_eq!(a.check_call(300, 0), COMLINE_OK);
        let call = a.request(300, 0, b"", true, b"args", 256);
        assert_eq!(call.status, COMLINE_OK);
        assert_eq!(call.bytes[..4], [0, 0xac, 0x02, 0]);
        let at_b = b.feed(&call.bytes);
        assert_eq!(
            (at_b.status, at_b.ev.kind, at_b.ev.protocol, at_b.ev.call_id),
            (COMLINE_OK, COMLINE_EVENT_REQUEST, 300, 0)
        );
        let answer = b.reply(300, at_b.ev.request_id, COMLINE_ENVELOPE_OK, 0, b"ok", 256);
        assert_eq!(a.feed(&answer.bytes).ev.kind, COMLINE_EVENT_REPLY);

        assert_eq!(a.outcome().unwrap(), "accepted/0/3/0/-/0:a:-,300:a:-/-");
        assert_eq!(a.warnings().unwrap(), Vec::<u16>::new());
    }
}

#[test]
fn it_runs_in_stream_mode_behind_length_prefixes() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let setup = |side| {
        let config = comline2_config(&hello, side, COMLINE_EXCHANGE_OFFER);
        let s = Setup {
            handshake: COMLINE_HANDSHAKE_COMLINE2,
            stream: true,
            ..Setup::plain("datagram@2")
        };
        let mut conn = ptr::null_mut();
        assert_eq!(
            unsafe { comline_connection_new_comline2(&s.config(), &config, &mut conn) },
            COMLINE_OK
        );
        Conn(conn)
    };
    let (mut a, mut b) = (setup(COMLINE_SIDE_INITIATOR), setup(COMLINE_SIDE_RESPONDER));
    let offer = a.start(4096);
    assert_eq!(offer.bytes[..4], ((hello.len()) as u32).to_le_bytes());
    assert_eq!(offer.bytes[4..], hello);
    assert_eq!(b.feed(&offer.bytes[..3]).status, COMLINE_NEED_MORE);
    let fed = b.feed(&offer.bytes);
    assert_eq!((fed.status, fed.consumed), (COMLINE_OK, offer.bytes.len()));
    assert_eq!(fed.ev.kind, COMLINE_EVENT_HANDSHAKE_CONTINUE);
    assert_ne!(fed.ev.flags & COMLINE_EVENT_SEND, 0);
    assert_eq!(run(&mut b, &mut a), (COMLINE_OK, COMLINE_OK));
    assert_eq!(a.request(0, 1, b"", true, b"", 256).status, COMLINE_OK);
}

/// Each event says where the handshake is, and requests wait for the last
/// message, on both exchanges.
#[test]
fn events_and_sends_follow_the_exchange() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let (mut a, mut b) = ends(Exchange::Offer, &hello, &hello, |_| {});
    // The responder has nothing to say first, and the initiator offers first.
    let nothing = b.start(4096);
    assert_eq!((nothing.status, nothing.written), (COMLINE_OK, 0));
    assert_eq!(a.feed(b"early").status, COMLINE_ERR_STATE);
    assert_eq!(a.feed(b"early").consumed, 0);

    let offer = a.start(4096);
    let fed = b.feed(&offer.bytes);
    assert_eq!(fed.ev.kind, COMLINE_EVENT_HANDSHAKE_CONTINUE);
    assert_ne!(fed.ev.flags & COMLINE_EVENT_SEND, 0);
    let answer = b.start(4096);
    let fed = a.feed(&answer.bytes);
    assert_eq!(fed.ev.kind, COMLINE_EVENT_HANDSHAKE_OK);
    assert_ne!(fed.ev.flags & COMLINE_EVENT_SEND, 0);
    assert_eq!(
        a.request(0, 0, b"", true, b"", 256).status,
        COMLINE_ERR_STATE,
        "the verdict is due"
    );
    let verdict = a.start(4096);
    assert_eq!(a.request(0, 0, b"", true, b"", 256).status, COMLINE_OK);
    let fed = b.feed(&verdict.bytes);
    assert_eq!((fed.ev.kind, fed.ev.flags), (COMLINE_EVENT_HANDSHAKE_OK, 0));
    // Nothing more to write, and asking is harmless.
    assert_eq!(a.start(4096).written, 0);

    // Symmetric, with the peer's hello read before this end's is written: the
    // hello still goes first.
    let hello = facts(Exchange::Symmetric, (0, 3, 0), BOTH);
    let (mut a, mut b) = ends(Exchange::Symmetric, &hello, &hello, |_| {});
    let from_b = b.start(4096);
    let fed = a.feed(&from_b.bytes);
    assert_eq!(fed.ev.kind, COMLINE_EVENT_HANDSHAKE_CONTINUE);
    assert_eq!(a.start(4096).bytes, hello);
    let verdict = a.start(4096);
    assert!(Message::decode(&verdict.bytes).is_ok());
    assert_eq!(a.start(4096).written, 0);
}

#[test]
fn a_newer_consumer_degrades_and_the_call_fails_locally() {
    let newer = facts(Exchange::Offer, (0, 4, 0), &[(0, 1, 3, &[0, 1, 2, 3])]);
    let older = facts(Exchange::Offer, (0, 3, 0), &[(0, 1, 1, &[0, 1, 2])]);
    let (mut a, mut b) = ends(Exchange::Offer, &newer, &older, |_| {});
    assert_eq!(run(&mut a, &mut b), (COMLINE_OK, COMLINE_OK));
    assert_eq!(a.check_call(0, 2), COMLINE_OK);
    assert_eq!(a.check_call(0, 3), COMLINE_ERR_UNAVAILABLE);
    assert_eq!(
        a.check_call(9, 0),
        COMLINE_ERR_UNAVAILABLE,
        "a protocol it doesn't have"
    );
    assert_eq!(a.peer_lacks(0).unwrap(), [3]);
    assert_eq!(
        a.peer_version().unwrap().0,
        comline_version {
            major: 0,
            minor: 3,
            patch: 0
        }
    );
    // The older end has nothing the peer lacks.
    assert_eq!(b.check_call(0, 2), COMLINE_OK);
    assert_eq!(b.peer_lacks(0).unwrap(), Vec::<u16>::new());
    assert_eq!(b.peer_version().unwrap().0.minor, 4);
}

#[test]
fn a_refusal_reaches_both_ends_with_its_reason_and_side() {
    let one = facts(Exchange::Offer, (0, 4, 0), &[(0, 1, 3, &[0])]);
    let other = facts(Exchange::Offer, (0, 5, 0), &[(0, 2, 4, &[0])]);
    let package = |c: &mut comline_comline2_config| c.scope = COMLINE_SCOPE_PACKAGE;
    let (mut a, mut b) = ends(Exchange::Offer, &one, &other, package);

    let offer = a.start(4096);
    let fed = b.feed(&offer.bytes);
    assert_eq!(fed.status, COMLINE_ERR_REFUSED);
    assert_ne!(fed.ev.flags & COMLINE_EVENT_SEND, 0, "the peer is told");
    assert_eq!(
        b.feed(&offer.bytes).status,
        COMLINE_ERR_STATE,
        "a refusal only sends"
    );
    let want = comline_refusal {
        reason: COMLINE_REASON_LINE_MISMATCH,
        side: COMLINE_SIDE_RESPONDER,
        has_protocol: 1,
        protocol: 0,
        ..Default::default()
    };
    assert_eq!(b.refusal().unwrap(), want);
    let answer = b.start(4096);
    assert_eq!(answer.status, COMLINE_OK);

    // Closed now, and the refusal can still be read.
    assert_eq!(b.start(4096).status, COMLINE_ERR_STATE);
    assert_eq!(b.refusal().unwrap(), want);
    assert_eq!(b.check_call(0, 0), COMLINE_ERR_STATE);

    let fed = a.feed(&answer.bytes);
    assert_eq!(
        (fed.status, fed.ev.flags & COMLINE_EVENT_SEND),
        (COMLINE_ERR_REFUSED, 0)
    );
    assert_eq!(
        a.refusal().unwrap(),
        want,
        "the same refusal, naming the same side"
    );
    assert_eq!(
        a.request(0, 0, b"", true, b"", 256).status,
        COMLINE_ERR_STATE
    );
}

#[test]
fn a_refused_protocol_leaves_the_others_and_says_why() {
    let one = facts(
        Exchange::Offer,
        (0, 4, 0),
        &[(0, 1, 3, &[0]), (1, 1, 3, &[0])],
    );
    let other = facts(
        Exchange::Offer,
        (0, 5, 0),
        &[(0, 2, 4, &[0]), (1, 1, 3, &[0])],
    );
    let (mut a, mut b) = ends(Exchange::Offer, &one, &other, |_| {});
    assert_eq!(run(&mut a, &mut b), (COMLINE_OK, COMLINE_OK));
    assert_eq!(a.check_call(0, 0), COMLINE_ERR_UNAVAILABLE);
    assert_eq!(a.check_call(1, 0), COMLINE_OK);
    assert_eq!(
        a.protocols().unwrap(),
        [
            comline_protocol_outcome {
                number: 0,
                reason: COMLINE_REASON_LINE_MISMATCH,
                _reserved: 0
            },
            comline_protocol_outcome {
                number: 1,
                reason: 0,
                _reserved: 0
            },
        ]
    );
    assert_eq!(
        a.peer_lacks(0).unwrap(),
        Vec::<u16>::new(),
        "a refused protocol lacks nothing"
    );
}

#[test]
fn a_shape_differing_at_the_same_version_refuses_or_warns() {
    let one = facts(Exchange::Symmetric, (0, 3, 0), &[(0, 1, 1, &[0])]);
    let other = facts(Exchange::Symmetric, (0, 3, 0), &[(0, 1, 2, &[0])]);

    let (mut a, mut b) = ends(Exchange::Symmetric, &one, &other, |_| {});
    run(&mut a, &mut b);
    assert_eq!(
        a.refusal().unwrap().reason,
        COMLINE_REASON_SAME_VERSION_DIFFERS
    );

    let warn = |c: &mut comline_comline2_config| {
        c.same_version_differs = COMLINE_SAME_VERSION_DIFFERS_WARN
    };
    let (mut a, mut b) = ends(Exchange::Symmetric, &one, &other, warn);
    assert_eq!(run(&mut a, &mut b), (COMLINE_OK, COMLINE_OK));
    assert_eq!(a.warnings().unwrap(), [0]);
    assert_eq!(b.warnings().unwrap(), [0]);
    assert_eq!(a.check_call(0, 0), COMLINE_OK);
}

#[test]
fn another_handshakes_frame_is_refused_as_another_version() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let (_, mut b) = ends(Exchange::Offer, &hello, &hello, |_| {});
    let frame = handshake_encode(1, "json@1", "datagram@2", 0, 64);
    assert_eq!(b.feed(&frame.bytes).status, COMLINE_ERR_REFUSED);
    assert_eq!(
        b.refusal().unwrap().reason,
        COMLINE_REASON_HANDSHAKE_VERSION
    );
}

#[test]
fn the_readers_wait_for_a_conclusion() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let (mut a, _) = ends(Exchange::Offer, &hello, &hello, |_| {});
    assert_eq!(a.check_call(0, 0), COMLINE_ERR_STATE);
    assert_eq!(a.warnings(), Err(COMLINE_ERR_STATE));
    assert_eq!(a.protocols(), Err(COMLINE_ERR_STATE));
    assert_eq!(a.peer_lacks(0), Err(COMLINE_ERR_STATE));
    assert_eq!(a.peer_version().err(), Some(COMLINE_ERR_STATE));
    assert_eq!(a.refusal(), Err(COMLINE_ERR_STATE));

    // Other handshakes have nothing to read, and no call is unavailable.
    let mut plain = Conn::new(&Setup::plain("datagram@3")).unwrap();
    assert_eq!(plain.check_call(0, 0), COMLINE_OK);
    assert_eq!(plain.warnings(), Err(COMLINE_ERR_STATE));
    assert_eq!(plain.refusal(), Err(COMLINE_ERR_STATE));
}

#[test]
fn buffer_too_small_reports_the_size_and_changes_nothing() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let (mut a, mut b) = ends(Exchange::Offer, &hello, &hello, |_| {});
    let small = a.start(4);
    assert_eq!(
        (small.status, small.written),
        (COMLINE_BUFFER_TOO_SMALL, hello.len())
    );
    let offer = a.start(hello.len());
    assert_eq!(offer.bytes, hello, "the retry writes it");
    assert_eq!(
        run(&mut b, &mut a),
        (COMLINE_OK, COMLINE_OK),
        "only `b` hasn't read it"
    );

    let two = facts(
        Exchange::Symmetric,
        (0, 3, 0),
        &[(0, 1, 1, &[0]), (1, 1, 1, &[0])],
    );
    let (mut a, mut b) = ends(Exchange::Symmetric, &two, &two, |_| {});
    run(&mut a, &mut b);
    let mut one = [comline_protocol_outcome::default()];
    let mut written = 0;
    let status = unsafe { comline_connection_protocols(a.0, one.as_mut_ptr(), 1, &mut written) };
    assert_eq!((status, written), (COMLINE_BUFFER_TOO_SMALL, 2));
    assert_eq!(one[0], comline_protocol_outcome::default());
}

#[test]
fn a_frame_over_the_limit_is_refused_when_written() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let config = comline2_config(&hello, COMLINE_SIDE_INITIATOR, COMLINE_EXCHANGE_OFFER);
    let s = Setup {
        handshake: COMLINE_HANDSHAKE_COMLINE2,
        max_frame: 8,
        ..Setup::plain("datagram@3")
    };
    let mut conn = ptr::null_mut();
    assert_eq!(
        unsafe { comline_connection_new_comline2(&s.config(), &config, &mut conn) },
        COMLINE_OK
    );
    let mut conn = Conn(conn);
    assert_eq!(conn.start(4096).status, COMLINE_ERR_FRAME_TOO_LARGE);
}

#[test]
fn bad_arguments_are_refused() {
    let invalid = COMLINE_ERR_INVALID_ARGUMENT;
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let good = comline2_config(&hello, COMLINE_SIDE_INITIATOR, COMLINE_EXCHANGE_OFFER);

    // A field outside its values, and a hello that isn't one.
    let bad: [fn(&mut comline_comline2_config); 6] = [
        |c| c.side = 2,
        |c| c.scope = 2,
        |c| c.newer_consumer = 2,
        |c| c.same_version_differs = 2,
        |c| c.exchange = 2,
        |c| c._reserved = [0, 1, 0],
    ];
    for tweak in bad {
        let mut config = good;
        tweak(&mut config);
        assert_eq!(Conn::comline2("datagram@3", &config).err(), Some(invalid));
    }
    for not in [&b"CO"[..], &[], &[0x43, 0x4f, 0x02, 0x03, 0x00, 0x00]] {
        let config = comline2_config(not, COMLINE_SIDE_INITIATOR, COMLINE_EXCHANGE_OFFER);
        assert_eq!(Conn::comline2("datagram@3", &config).err(), Some(invalid));
    }
    assert_eq!(
        Conn::comline2("datagram@1", &good).err(),
        Some(COMLINE_ERR_UNKNOWN_FRAMING)
    );

    // Each constructor takes its own handshakes only.
    let comline2 = Setup {
        handshake: COMLINE_HANDSHAKE_COMLINE2,
        ..Setup::plain("datagram@3")
    };
    assert_eq!(Conn::new(&comline2).err(), Some(invalid));
    let mut out = ptr::null_mut();
    let none = Setup::plain("datagram@3");
    assert_eq!(
        unsafe { comline_connection_new_comline2(&none.config(), &good, &mut out) },
        invalid
    );
    assert!(out.is_null());
    assert_eq!(
        unsafe { comline_connection_new_comline2(&comline2.config(), ptr::null(), &mut out) },
        invalid
    );
    assert!(out.is_null());
    assert_eq!(
        unsafe { comline_connection_new_comline2(&comline2.config(), &good, ptr::null_mut()) },
        invalid
    );
    assert_eq!(
        unsafe { comline_connection_new_comline2(ptr::null(), &good, &mut out) },
        invalid
    );

    // A null handle, and null outputs.
    let null = ptr::null_mut();
    let mut written = 0;
    let mut numbers = [0u16; 2];
    unsafe {
        assert_eq!(comline_connection_check_call(null, 0, 0), invalid);
        assert_eq!(
            comline_connection_refusal(null, &mut comline_refusal::default()),
            invalid
        );
        assert_eq!(
            comline_connection_warnings(null, numbers.as_mut_ptr(), 2, &mut written),
            invalid
        );
        assert_eq!(
            comline_connection_peer_lacks(null, 0, numbers.as_mut_ptr(), 2, &mut written),
            invalid
        );
        assert_eq!(
            comline_connection_protocols(null, ptr::null_mut(), 0, &mut written),
            invalid
        );
        assert_eq!(
            comline_connection_peer_version(
                null,
                &mut comline_version::default(),
                ptr::null_mut(),
                0,
                &mut written
            ),
            invalid
        );
    }
    let (mut a, mut b) = ends(Exchange::Offer, &hello, &hello, |_| {});
    run(&mut a, &mut b);
    unsafe {
        assert_eq!(comline_connection_refusal(a.0, ptr::null_mut()), invalid);
        assert_eq!(
            comline_connection_warnings(a.0, ptr::null_mut(), 1, &mut written),
            invalid
        );
        assert_eq!(
            comline_connection_warnings(a.0, numbers.as_mut_ptr(), 2, ptr::null_mut()),
            invalid
        );
        assert_eq!(
            comline_connection_peer_lacks(a.0, 0, ptr::null_mut(), 1, &mut written),
            invalid
        );
        assert_eq!(
            comline_connection_protocols(a.0, ptr::null_mut(), 1, &mut written),
            invalid
        );
        assert_eq!(
            comline_connection_warnings(a.0, numbers.as_mut_ptr(), usize::MAX, &mut written),
            invalid
        );
        assert_eq!(
            comline_connection_peer_version(a.0, ptr::null_mut(), ptr::null_mut(), 0, &mut written),
            invalid
        );
    }
    let (mut kind, mut reason) = (0, 0);
    unsafe {
        assert_eq!(
            comline_comline2_message_check(ptr::null(), 1, &mut kind, &mut reason),
            invalid
        );
        assert_eq!(
            comline_comline2_message_check(
                hello.as_ptr(),
                hello.len(),
                ptr::null_mut(),
                &mut reason
            ),
            invalid
        );
        assert_eq!(
            comline_comline2_message_check(hello.as_ptr(), hello.len(), &mut kind, ptr::null_mut()),
            invalid
        );
    }
}

#[test]
fn overlapping_inputs_and_outputs_are_refused() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let (mut a, mut b) = ends(Exchange::Offer, &hello, &hello, |_| {});
    run(&mut a, &mut b);
    let invalid = COMLINE_ERR_INVALID_ARGUMENT;
    #[repr(align(8))]
    struct Aligned([u16; 16]);
    let mut shared = Aligned([0; 16]);
    let at = shared.0.as_mut_ptr();
    unsafe {
        assert_eq!(
            comline_connection_warnings(a.0, at, 8, at.cast::<usize>()),
            invalid
        );
        assert_eq!(
            comline_connection_peer_lacks(a.0, 0, at, 8, at.cast::<usize>()),
            invalid
        );
        assert_eq!(
            comline_connection_protocols(a.0, at.cast(), 2, at.cast::<usize>()),
            invalid
        );
        assert_eq!(
            comline_connection_peer_version(
                a.0,
                at.cast(),
                at.cast::<u8>().add(4),
                4,
                at.cast::<usize>().add(1)
            ),
            invalid
        );
        // The struct the handshake is read from, and the handle out over it.
        let config = comline2_config(&hello, COMLINE_SIDE_INITIATOR, COMLINE_EXCHANGE_OFFER);
        let s = Setup {
            handshake: COMLINE_HANDSHAKE_COMLINE2,
            ..Setup::plain("datagram@3")
        };
        let mut overlaid = config;
        let out = (&mut overlaid as *mut comline_comline2_config).cast();
        assert_eq!(
            comline_connection_new_comline2(&s.config(), &overlaid, out),
            invalid
        );
    }
}

#[test]
fn a_message_is_checked_alone() {
    let hello = facts(Exchange::Offer, (0, 3, 0), BOTH);
    let (mut kind, mut reason) = (9, 9);
    let status = unsafe {
        comline_comline2_message_check(hello.as_ptr(), hello.len(), &mut kind, &mut reason)
    };
    assert_eq!(
        (status, kind, reason),
        (COMLINE_OK, COMLINE_COMLINE2_HELLO, 0)
    );
    let frame = handshake_encode(1, "json@1", "datagram@2", 0, 64);
    let status = unsafe {
        comline_comline2_message_check(
            frame.bytes.as_ptr(),
            frame.bytes.len(),
            &mut kind,
            &mut reason,
        )
    };
    assert_eq!(
        (status, kind, reason),
        (COMLINE_ERR_HANDSHAKE, 0, COMLINE_REASON_HANDSHAKE_VERSION)
    );
    let status = unsafe { comline_comline2_message_check(ptr::null(), 0, &mut kind, &mut reason) };
    assert_eq!(
        (status, kind, reason),
        (COMLINE_ERR_HANDSHAKE, 0, COMLINE_REASON_MALFORMED)
    );
}

/// Whatever is fed during the handshake, nothing panics, the status is a
/// documented one, and a later call still answers. Every byte under Miri is
/// too slow, so there it takes a sample.
#[test]
fn any_bytes_during_the_handshake_are_refused_cleanly() {
    let hello = facts(Exchange::Symmetric, (0, 3, 0), BOTH);
    for side in [COMLINE_SIDE_INITIATOR, COMLINE_SIDE_RESPONDER] {
        for flip in (0..hello.len()).step_by(if cfg!(miri) { 17 } else { 1 }) {
            let config = comline2_config(&hello, side, COMLINE_EXCHANGE_SYMMETRIC);
            let mut conn = Conn::comline2("datagram@3", &config).unwrap();
            conn.start(4096);
            let mut bad = hello.clone();
            bad[flip] ^= 0xff;
            let fed = conn.feed(&bad);
            assert!(
                matches!(fed.status, COMLINE_OK | COMLINE_ERR_REFUSED),
                "byte {flip}: {}",
                fed.status
            );
            assert!(matches!(
                conn.start(4096).status,
                COMLINE_OK | COMLINE_ERR_STATE
            ));
            let mut truncated = hello.clone();
            truncated.truncate(flip);
            let config = comline2_config(&hello, side, COMLINE_EXCHANGE_SYMMETRIC);
            let mut conn = Conn::comline2("datagram@3", &config).unwrap();
            conn.start(4096);
            assert_eq!(
                conn.feed(&truncated).status,
                COMLINE_ERR_REFUSED,
                "cut at {flip}"
            );
            assert_eq!(
                conn.refusal().unwrap().reason,
                COMLINE_REASON_MALFORMED,
                "cut at {flip}"
            );
        }
    }
}
