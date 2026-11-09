// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline@2` on a connection, fed what a peer may send. The input is a byte of
//! options (bit 0 the side, bit 1 the exchange, bit 2 the scope, bit 3
//! `newer_consumer`, bit 4 `same_version_differs`, bit 5 stream mode), then frames,
//! each a `u16` LE length and that many bytes, as far as the input goes.
//!
//! No panic may cross and no handle be poisoned; every status must be one the
//! function documents; `COMLINE_EVENT_SEND` must mean `start` has a message,
//! and no message must be left unannounced; and once it's concluded or refused,
//! the functions that read the outcome must answer.

#![no_main]

use std::ptr;

use comline_runtime::handshake::comline2::{
    Encoding, EncodingKind, Exchange, Hash128, Hello, Message, PackageVersion, Protocol, Release,
};
use comline_runtime_ffi::*;
use libfuzzer_sys::fuzz_target;

const OUT: usize = 1 << 16;

/// What the vectors' local end is, so the seeds reach past the first checks.
fn facts(exchange: Exchange) -> Vec<u8> {
    let hash = |byte| Hash128([byte; 16]);
    let protocol = |number, minor, shape, functions: &[u16]| Protocol {
        number,
        line: Release::new(0, minor, 0),
        shape_hash: hash(shape),
        functions: functions.to_vec(),
    };
    Message::Hello(Hello {
        exchange,
        package_id: Hash128::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
        version: PackageVersion::release(0, 3, 0),
        encodings: vec![
            Encoding { kind: EncodingKind::Format, param_hash: hash(0x11) },
            Encoding { kind: EncodingKind::Framing, param_hash: hash(0x22) },
            Encoding { kind: EncodingKind::Handshake, param_hash: hash(0x33) },
        ],
        protocols: vec![protocol(0, 2, 0xa0, &[0, 1, 2]), protocol(1, 1, 0xb0, &[0, 1])],
    })
    .to_bytes()
}

fuzz_target!(|input: &[u8]| {
    let Some((&options, mut rest)) = input.split_first() else {
        return;
    };
    let bit = |n: u8| (options >> n) & 1;
    let hello = facts(if bit(1) == 1 { Exchange::Symmetric } else { Exchange::Offer });
    let stream = bit(5);
    let config = comline_config {
        ir_hash: 0,
        wire_format: b"packed@1".as_ptr(),
        wire_format_len: 8,
        framing: b"datagram@3".as_ptr(),
        framing_len: 10,
        max_frame: 0,
        capabilities: 0,
        max_waiting: 4,
        handshake: COMLINE_HANDSHAKE_COMLINE2,
        stream,
        unordered: 0,
        _reserved: 0,
    };
    let comline2 = comline_comline2_config {
        hello: hello.as_ptr(),
        hello_len: hello.len(),
        side: bit(0),
        scope: bit(2),
        newer_consumer: bit(3),
        same_version_differs: bit(4),
        exchange: bit(1),
        _reserved: [0; 3],
    };
    let mut conn = ptr::null_mut();
    let status = unsafe { comline_connection_new_comline2(&config, &comline2, &mut conn) };
    assert_eq!(status, COMLINE_OK);
    let mut out = vec![0u8; OUT];

    let send = |conn, out: &mut [u8]| {
        let mut written = usize::MAX;
        let status = unsafe { comline_connection_start(conn, out.as_mut_ptr(), OUT, &mut written) };
        assert!(matches!(status, COMLINE_OK | COMLINE_ERR_STATE), "start gave {status}");
        (status, if status == COMLINE_OK { written } else { 0 })
    };
    send(conn, &mut out);

    while rest.len() >= 2 {
        let len = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        let body = &rest[2..];
        let frame = &body[..len.min(body.len())];
        rest = &body[frame.len()..];

        let mut framed = Vec::new();
        if stream == 1 {
            framed.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        }
        framed.extend_from_slice(frame);

        let mut consumed = usize::MAX;
        let mut ev = comline_event::default();
        let status =
            unsafe { comline_connection_feed(conn, framed.as_ptr(), framed.len(), &mut consumed, &mut ev) };
        assert!(
            matches!(
                status,
                COMLINE_OK | COMLINE_ERR_REFUSED | COMLINE_ERR_STATE | COMLINE_ERR_FRAMING | COMLINE_ERR_PEER_REFUSED
            ),
            "feed gave {status}"
        );
        if ev.flags & COMLINE_EVENT_SEND != 0 {
            let (status, written) = send(conn, &mut out);
            assert_eq!(status, COMLINE_OK, "a message was announced");
            assert_ne!(written, 0, "a message was announced");
        }
    }

    // The readers answer, whatever it came to.
    let concluded = [COMLINE_OK, COMLINE_ERR_STATE, COMLINE_ERR_UNAVAILABLE];
    let status = unsafe { comline_connection_check_call(conn, 0, 0) };
    assert!(concluded.contains(&status), "check_call gave {status}");
    let mut refusal = comline_refusal::default();
    let status = unsafe { comline_connection_refusal(conn, &mut refusal) };
    assert!(matches!(status, COMLINE_OK | COMLINE_ERR_STATE), "refusal gave {status}");
    if status == COMLINE_OK {
        assert!(reason_known(refusal.reason), "reason {}", refusal.reason);
    }
    let mut numbers = [0u16; 8];
    let mut written = 0;
    let status = unsafe { comline_connection_warnings(conn, numbers.as_mut_ptr(), 8, &mut written) };
    assert!(matches!(status, COMLINE_OK | COMLINE_ERR_STATE | COMLINE_BUFFER_TOO_SMALL), "warnings gave {status}");
    unsafe { comline_connection_free(conn) };
});

fn reason_known(code: u8) -> bool {
    (COMLINE_REASON_MALFORMED..=COMLINE_REASON_UNKNOWN_PROTOCOL).contains(&code)
}
