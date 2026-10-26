// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The conformance vectors, replayed through the C ABI only: every case in
//! `name-hash`, `handshake`, `envelope` and `framing-{datagram,datagram2,
//! jsonrpc}`, against the expectations `runtime/conformance` holds the Rust
//! runtime to (exact bytes, `"expect": "reject"`, `"compare": "json"`).
//!
//! The ABI is a connection's, not the framings' one by one, so each op is
//! mapped onto it:
//!
//! - `name_hash` → `comline_name_hash`.
//! - `handshake` → `comline_handshake_encode` gives the bytes;
//!   `comline_handshake_check` reads them back (the peer frame it returns
//!   must hold the input's hashes and bits). For a connection framing, a
//!   connection's `start` must write the same bytes and `feed` must take
//!   them. A rejection must be `COMLINE_ERR_HANDSHAKE` both ways.
//! - `envelope` → there is no bare-envelope call: an envelope is the tail of
//!   a `datagram@2` reply. Encoding is `encode_reply`, with the
//!   10-byte reply header (kind, protocol 0, request id 0) cut off; decoding
//!   is `feed` of that header plus the vector's bytes, on a connection
//!   waiting on request 0.
//! - `request` / `response` on `datagram@2` and `jsonrpc@1` →
//!   `encode_request` / `encode_reply` and `feed`. A connection hands out
//!   its own request ids, so it is driven to the vector's id by spending ids
//!   on one-way calls; an id too large to drive to (`u64::MAX`) is encoded
//!   at the connection's own id and compared with that id field set to the
//!   vector's, and its decoding is still exact. A rejection must fail
//!   `feed`, or read as the other kind of frame: a connection reads every
//!   frame as a request or a reply, so "not a request" can be a reply (a
//!   JSON-RPC message without `method` is one) and the other way round.
//! - `datagram@1` is the lockstep framing, and a connection always speaks
//!   the connection framings, so none of its cases map onto the ABI. They
//!   are listed, and the ABI is checked to refuse `datagram@1`.

mod common;

use std::collections::BTreeMap;

use comline_runtime_conformance::{json_equal, Case, Compare, Expect, Op, Value, Vectors, Wire};
use comline_runtime_ffi::*;
use common::*;

const FILES: [&str; 6] = [
    "name-hash",
    "handshake",
    "envelope",
    "framing-datagram",
    "framing-datagram2",
    "framing-jsonrpc",
];

/// The largest request id a connection is driven to by spending ids.
const DRIVABLE: u64 = 1_000;

/// The vectors, compiled in rather than read at run time, so the replay
/// also runs under Miri's isolation (no file system).
fn vectors(feature: &str) -> Vectors {
    macro_rules! file {
        ($name:literal) => {
            (
                $name,
                include_str!(concat!("../../conformance/vectors/", $name, ".json")),
            )
        };
    }
    const FILES_TEXT: [(&str, &str); 6] = [
        file!("name-hash"),
        file!("handshake"),
        file!("envelope"),
        file!("framing-datagram"),
        file!("framing-datagram2"),
        file!("framing-jsonrpc"),
    ];
    let (_, text) = FILES_TEXT
        .iter()
        .find(|(name, _)| *name == feature)
        .unwrap_or_else(|| panic!("no vectors for {feature}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{feature}: {e}"))
}

/// How a case went through the ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ran {
    /// Every check the conformance harness makes, through the ABI.
    Full,
    /// Encoded at the connection's own request id and compared with the id
    /// set to the vector's; decoded exactly.
    IdPatched,
    /// A rejection, which the connection read as the other kind of frame.
    RejectedAsOtherKind(&'static str),
    /// No ABI operation does it.
    NotMapped(&'static str),
}

#[test]
fn every_case_replays_through_the_abi() {
    let mut ran: BTreeMap<String, Ran> = BTreeMap::new();
    let mut failures = Vec::new();
    for file in FILES {
        let vectors = vectors(file);
        assert!(!vectors.cases.is_empty(), "{file} has no cases");
        for case in &vectors.cases {
            match std::panic::catch_unwind(|| replay(case)) {
                Ok(Ok(how)) => {
                    ran.insert(case.id.clone(), how);
                }
                Ok(Err(why)) => failures.push(format!("{}: {why}", case.id)),
                Err(_) => failures.push(format!("{}: panicked", case.id)),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    // What didn't go through every check, so a change here is seen.
    let partial: Vec<_> = ran.iter().filter(|(_, how)| **how != Ran::Full).collect();
    for (id, how) in &partial {
        eprintln!("{id}: {how:?}");
    }
    let count = |want: fn(&Ran) -> bool| ran.values().filter(|r| want(r)).count();
    eprintln!(
        "{} cases: {} full, {} id-patched, {} rejected as the other kind, {} not mapped",
        ran.len(),
        count(|r| *r == Ran::Full),
        count(|r| *r == Ran::IdPatched),
        count(|r| matches!(r, Ran::RejectedAsOtherKind(_))),
        count(|r| matches!(r, Ran::NotMapped(_))),
    );
    let not_mapped: Vec<_> = ran
        .iter()
        .filter(|(_, how)| matches!(how, Ran::NotMapped(_)))
        .map(|(id, _)| id.as_str())
        .collect();
    assert!(
        not_mapped
            .iter()
            .all(|id| id.starts_with("framing-datagram/")),
        "only datagram@1 is outside the ABI: {not_mapped:?}"
    );
    assert_eq!(
        not_mapped.len(),
        vectors("framing-datagram").cases.len(),
        "every datagram@1 case is listed"
    );
    let patched: Vec<_> = ran
        .iter()
        .filter(|(_, how)| **how == Ran::IdPatched)
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        patched,
        [
            "framing-datagram2/request-large-ids",
            "framing-jsonrpc/request-large-id"
        ]
    );
}

#[test]
fn datagram1_is_refused_on_a_connection() {
    assert!(!supports("datagram@1"));
    assert_eq!(
        Conn::new(&Setup::plain("datagram@1")).err(),
        Some(COMLINE_ERR_UNKNOWN_FRAMING)
    );
}

fn replay(case: &Case) -> Result<Ran, String> {
    let input = &case.input;
    let framing = input["framing"].as_str();
    match (case.op, framing) {
        // The free handshake functions take any name, `datagram@1` too.
        (Op::NameHash, _) => name_hash_case(case),
        (Op::Handshake, _) => handshake_case(case),
        (_, Some("datagram@1")) => Ok(Ran::NotMapped(
            "datagram@1 is lockstep; a connection speaks only the connection framings",
        )),
        (Op::Envelope, _) => envelope_case(case),
        (Op::Request, Some(f @ ("datagram@2" | "jsonrpc@1"))) => request_case(case, leak(f)),
        (Op::Response, Some(f @ ("datagram@2" | "jsonrpc@1"))) => response_case(case, leak(f)),
        _ => Err(format!("no mapping for {:?} on {framing:?}", case.op)),
    }
}

fn leak(s: &str) -> &'static str {
    match s {
        "datagram@2" => "datagram@2",
        "jsonrpc@1" => "jsonrpc@1",
        _ => unreachable!(),
    }
}

fn check(ok: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(why())
    }
}

fn same(case: &Case, encoded: &[u8]) -> bool {
    encoded == case.bytes.0.as_slice()
        || (case.compare == Compare::Json && json_equal(encoded, &case.bytes.0))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn u64_of(v: &serde_json::Value) -> u64 {
    v.as_str()
        .expect("a 64-bit integer is a decimal string")
        .parse()
        .expect("a decimal")
}

fn str_of(v: &serde_json::Value) -> &str {
    v.as_str().expect("a string")
}

fn bytes_of(v: &serde_json::Value) -> Vec<u8> {
    let text = str_of(v);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

/// A vector's params, as the host's `json@1` codec writes them.
fn params_of(input: &serde_json::Value) -> Result<Vec<u8>, String> {
    let format = str_of(&input["format"]);
    if format != "json@1" {
        return Err(format!("params in {format}: only json@1 is encoded here"));
    }
    let value: Value =
        serde_json::from_value(input["params"].clone()).map_err(|e| e.to_string())?;
    serde_json::to_vec(&Wire(&value)).map_err(|e| e.to_string())
}

// ── name_hash ──────────────────────────────────────────────────────────────

fn name_hash_case(case: &Case) -> Result<Ran, String> {
    let hash = name_hash(str_of(&case.input["name"]));
    check(hash.to_le_bytes() == case.bytes.0.as_slice(), || {
        format!("hash {hash:#x}, expected {}", hex(&case.bytes.0))
    })?;
    Ok(Ran::Full)
}

// ── handshake ──────────────────────────────────────────────────────────────

/// A valid frame to check a rejected one against.
fn some_frame() -> Vec<u8> {
    handshake_encode(1, "json@1", "datagram@2", 0, 64).bytes
}

fn handshake_case(case: &Case) -> Result<Ran, String> {
    let bytes = &case.bytes.0;
    if case.expect == Expect::Reject {
        let (status, peer) = handshake_check(&some_frame(), bytes);
        check(status == COMLINE_ERR_HANDSHAKE, || {
            format!("check gave {status}")
        })?;
        check(peer.is_none(), || {
            "a frame that doesn't parse gave a peer".into()
        })?;
        // And on a connection, as the first frame fed.
        let mut setup = Setup::plain("datagram@2");
        setup.handshake = COMLINE_HANDSHAKE_COMLINE;
        let mut conn = Conn::new(&setup).map_err(|s| format!("new: {s}"))?;
        let fed = conn.feed(bytes);
        check(fed.status == COMLINE_ERR_HANDSHAKE, || {
            format!("feed gave {}", fed.status)
        })?;
        check(conn.feed(bytes).status == COMLINE_ERR_STATE, || {
            "a refused handshake leaves the connection open".into()
        })?;
        return Ok(Ran::Full);
    }

    let input = &case.input;
    let ir_hash = u64_of(&input["ir_hash"]);
    let wire_format = str_of(&input["wire_format"]);
    let framing = str_of(&input["framing"]);
    let capabilities = input["capabilities"].as_u64().expect("bits") as u32;

    // Encode.
    let too_small = handshake_encode(ir_hash, wire_format, framing, capabilities, 30);
    check(
        too_small.status == COMLINE_BUFFER_TOO_SMALL && too_small.written == 31,
        || format!("30 bytes of room gave {too_small:?}"),
    )?;
    let encoded = handshake_encode(ir_hash, wire_format, framing, capabilities, 64);
    check(encoded.status == COMLINE_OK, || {
        format!("encode gave {}", encoded.status)
    })?;
    check(same(case, &encoded.bytes), || {
        format!("encodes to {}", hex(&encoded.bytes))
    })?;

    // Decode: the peer's frame read back holds the input.
    let (status, peer) = handshake_check(&encoded.bytes, bytes);
    check(status == COMLINE_OK, || format!("check gave {status}"))?;
    let want = comline_handshake {
        ir_hash,
        wire_format: name_hash(wire_format),
        framing: name_hash(framing),
        capabilities,
        _reserved: 0,
    };
    check(peer == Some(want), || format!("decodes to {peer:?}"))?;

    // On a connection, when the framing is a connection framing.
    if supports(framing) {
        let mut setup = Setup::plain(leak(framing));
        setup.handshake = COMLINE_HANDSHAKE_COMLINE;
        setup.ir_hash = ir_hash;
        setup.wire_format = ["json@1", "msgpack@1", "packed@1"]
            .into_iter()
            .find(|name| *name == wire_format)
            .ok_or_else(|| format!("no static name for {wire_format}"))?;
        setup.capabilities = capabilities;
        let mut conn = Conn::new(&setup).map_err(|s| format!("new: {s}"))?;
        let started = conn.start(64);
        check(
            started.status == COMLINE_OK && started.bytes == *bytes,
            || format!("start wrote {started:?}"),
        )?;
        let fed = conn.feed(bytes);
        check(
            fed.status == COMLINE_OK
                && fed.ev.kind == COMLINE_EVENT_HANDSHAKE_OK
                && fed.ev.capabilities == capabilities
                && fed.consumed == bytes.len(),
            || format!("feed gave {fed:?}"),
        )?;
    }
    Ok(Ran::Full)
}

// ── envelope ───────────────────────────────────────────────────────────────

/// A `datagram@2` reply's header for protocol 0, request 0.
const REPLY_HEADER: [u8; 10] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// A `datagram@2` connection waiting on request 0.
fn waiting_on_zero() -> Result<Conn, String> {
    let mut conn = Conn::new(&Setup::plain("datagram@2")).map_err(|s| format!("new: {s}"))?;
    let sent = conn.request(0, 0, b"", true, b"", 64);
    check(sent.status == COMLINE_OK && sent.request_id == 0, || {
        format!("request gave {sent:?}")
    })?;
    Ok(conn)
}

/// `(envelope, error_id, body)` from an `envelope` / `response` input.
fn reply_of(input: &serde_json::Value) -> (u8, u16, Vec<u8>) {
    match str_of(&input["kind"]) {
        "ok" => (COMLINE_ENVELOPE_OK, 0, bytes_of(&input["payload"])),
        "err" => (
            COMLINE_ENVELOPE_ERR,
            input["id"].as_u64().expect("an ordinal") as u16,
            bytes_of(&input["body"]),
        ),
        "fault" => (COMLINE_ENVELOPE_FAULT, 0, Vec::new()),
        other => panic!("kind {other}"),
    }
}

/// A reply event holds `(envelope, error_id, body)`.
fn check_reply(input: &[u8], fed: &Fed, want: &(u8, u16, Vec<u8>)) -> Result<(), String> {
    let (envelope, error_id, body) = want;
    check(
        fed.status == COMLINE_OK
            && fed.ev.kind == COMLINE_EVENT_REPLY
            && fed.ev.envelope == *envelope
            && fed.ev.error_id == *error_id
            && at(input, fed.ev.body) == body.as_slice()
            && fed.ev.flags & COMLINE_EVENT_BODY_ABSENT == 0,
        || format!("feed gave {fed:?}"),
    )
}

fn envelope_case(case: &Case) -> Result<Ran, String> {
    let mut frame = REPLY_HEADER.to_vec();
    frame.extend_from_slice(&case.bytes.0);

    if case.expect == Expect::Reject {
        let fed = waiting_on_zero()?.feed(&frame);
        check(fed.status == COMLINE_ERR_FRAMING, || {
            format!("feed gave {fed:?}")
        })?;
        return Ok(Ran::Full);
    }

    let want = reply_of(&case.input);
    let mut conn = Conn::new(&Setup::plain("datagram@2")).map_err(|s| format!("new: {s}"))?;
    let encoded = conn.reply(0, 0, want.0, want.1, &want.2, 256);
    check(
        encoded.status == COMLINE_OK && encoded.bytes.starts_with(&REPLY_HEADER),
        || format!("encode gave {encoded:?}"),
    )?;
    let envelope = &encoded.bytes[REPLY_HEADER.len()..];
    check(same(case, envelope), || {
        format!("encodes to {}", hex(envelope))
    })?;

    let fed = waiting_on_zero()?.feed(&frame);
    check_reply(&frame, &fed, &want)?;
    Ok(Ran::Full)
}

// ── request ────────────────────────────────────────────────────────────────

fn request_case(case: &Case, framing: &'static str) -> Result<Ran, String> {
    let bytes = &case.bytes.0;
    let mut conn = Conn::new(&Setup::plain(framing)).map_err(|s| format!("new: {s}"))?;

    if case.expect == Expect::Reject {
        let fed = conn.feed(bytes);
        if fed.status != COMLINE_OK {
            check(fed.status == COMLINE_ERR_FRAMING, || {
                format!("feed gave {}", fed.status)
            })?;
            return Ok(Ran::Full);
        }
        check(fed.ev.kind != COMLINE_EVENT_REQUEST, || {
            format!("read a request it must reject: {fed:?}")
        })?;
        return Ok(Ran::RejectedAsOtherKind("read as a reply"));
    }

    let input = &case.input;
    let request_id = u64_of(&input["request_id"]);
    let protocol = input["protocol"].as_u64().unwrap_or(0) as u16;
    let call_id = input["call_id"].as_u64().expect("a call id") as u16;
    let method = str_of(&input["method"]);
    let one_way = input["one_way"].as_bool().unwrap_or(false);
    let params = params_of(input)?;

    // Encode, at the vector's id when the connection can be driven to it.
    let patched = request_id > DRIVABLE;
    if !patched && !one_way {
        conn.spend_ids(0, request_id);
    }
    let encoded = conn.request(
        protocol,
        call_id,
        method.as_bytes(),
        !one_way,
        &params,
        4096,
    );
    check(encoded.status == COMLINE_OK, || {
        format!("encode gave {}", encoded.status)
    })?;
    let mut frame = encoded.bytes.clone();
    if patched {
        frame = with_request_id(framing, &frame, protocol, request_id)?;
    } else if !one_way {
        check(encoded.request_id == request_id, || {
            format!("encoded as request {}", encoded.request_id)
        })?;
    }
    check(same(case, &frame), || format!("encodes to {}", hex(&frame)))?;
    // A one-way call doesn't wait, and a call that expects a reply does.
    check(conn.is_waiting() == Ok(!one_way), || "waiting".into())?;

    // Decode, on a fresh connection.
    let mut peer = Conn::new(&Setup::plain(framing)).map_err(|s| format!("new: {s}"))?;
    let fed = peer.feed(bytes);
    check(
        fed.status == COMLINE_OK
            && fed.ev.kind == COMLINE_EVENT_REQUEST
            && fed.consumed == bytes.len()
            && at(bytes, fed.ev.frame) == bytes.as_slice()
            && fed.ev.request_id == request_id
            && (fed.ev.flags & COMLINE_EVENT_ONE_WAY != 0) == one_way
            && at(bytes, fed.ev.body) == params.as_slice(),
        || format!("feed gave {fed:?}"),
    )?;
    match framing {
        "datagram@2" => check(
            fed.ev.flags & COMLINE_EVENT_HAS_PROTOCOL != 0
                && fed.ev.flags & COMLINE_EVENT_CALL_BY_NAME == 0
                && fed.ev.protocol == protocol
                && fed.ev.call_id == call_id,
            || format!("feed gave {fed:?}"),
        )?,
        _ => {
            check(
                fed.ev.flags & COMLINE_EVENT_HAS_PROTOCOL == 0
                    && fed.ev.flags & COMLINE_EVENT_CALL_BY_NAME != 0,
                || format!("feed gave {fed:?}"),
            )?;
            let name = method_name(bytes, &fed.ev);
            check(name == method, || format!("decoded the method as {name:?}"))?;
        }
    }
    Ok(if patched { Ran::IdPatched } else { Ran::Full })
}

/// `frame` with its request id set to `id`: the id field of a `datagram@2`
/// request, or a JSON-RPC request's `id` member.
fn with_request_id(framing: &str, frame: &[u8], protocol: u16, id: u64) -> Result<Vec<u8>, String> {
    match framing {
        "datagram@2" => {
            let varint = match protocol {
                0..=0x7f => 1,
                0x80..=0x3fff => 2,
                _ => 3,
            };
            let at = 1 + varint + 2;
            let mut out = frame.to_vec();
            out[at..at + 8].copy_from_slice(&id.to_le_bytes());
            Ok(out)
        }
        _ => {
            let mut message: serde_json::Value =
                serde_json::from_slice(frame).map_err(|e| e.to_string())?;
            message["id"] = id.into();
            serde_json::to_vec(&message).map_err(|e| e.to_string())
        }
    }
}

// ── response ───────────────────────────────────────────────────────────────

fn response_case(case: &Case, framing: &'static str) -> Result<Ran, String> {
    let bytes = &case.bytes.0;

    if case.expect == Expect::Reject {
        let mut conn = Conn::new(&Setup::plain(framing)).map_err(|s| format!("new: {s}"))?;
        let fed = conn.feed(bytes);
        if fed.status != COMLINE_OK {
            check(fed.status == COMLINE_ERR_FRAMING, || {
                format!("feed gave {}", fed.status)
            })?;
            return Ok(Ran::Full);
        }
        check(
            fed.ev.kind != COMLINE_EVENT_REPLY && fed.ev.kind != COMLINE_EVENT_STALE,
            || format!("read a reply it must reject: {fed:?}"),
        )?;
        return Ok(Ran::RejectedAsOtherKind("read as a request"));
    }

    let input = &case.input;
    let request_id = u64_of(&input["request_id"]);
    let protocol = input["protocol"].as_u64().unwrap_or(0) as u16;
    let want = reply_of(input);

    // Encode: a reply carries the peer's request id, so nothing to drive.
    let mut conn = Conn::new(&Setup::plain(framing)).map_err(|s| format!("new: {s}"))?;
    let encoded = conn.reply(protocol, request_id, want.0, want.1, &want.2, 4096);
    check(encoded.status == COMLINE_OK, || {
        format!("encode gave {}", encoded.status)
    })?;
    check(same(case, &encoded.bytes), || {
        format!("encodes to {}", hex(&encoded.bytes))
    })?;

    // Decode, on a connection waiting on the vector's request id.
    check(request_id <= DRIVABLE, || {
        "a reply id too large to wait on".into()
    })?;
    let mut caller = Conn::new(&Setup::plain(framing)).map_err(|s| format!("new: {s}"))?;
    caller.spend_ids(0, request_id);
    let sent = caller.request(protocol, 0, b"call", true, b"null", 256);
    check(
        sent.status == COMLINE_OK && sent.request_id == request_id,
        || format!("request gave {sent:?}"),
    )?;
    let fed = caller.feed(bytes);
    check_reply(bytes, &fed, &want)?;
    check(fed.ev.request_id == request_id, || {
        format!("feed gave {fed:?}")
    })?;
    check(caller.is_waiting() == Ok(false), || "still waiting".into())?;
    Ok(Ran::Full)
}
