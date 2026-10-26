// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How a conformance case reads, for the ABI: the files that map onto it,
//! the fields of a case's input, its params as the host's codec writes them,
//! and the request id rules. Shared by `tests/vectors.rs`, which replays the
//! cases through the ABI from Rust, and `examples/export_vectors.rs`, which
//! writes them out for the C and Node harnesses, so both apply one set of
//! rules. Nothing here calls the ABI.

#![allow(dead_code)]

use comline_runtime_conformance::{Value, Vectors, Wire};
use comline_runtime_ffi::{COMLINE_ENVELOPE_ERR, COMLINE_ENVELOPE_FAULT, COMLINE_ENVELOPE_OK};

/// The files with cases on the ABI: every one a `comline_connection` or a
/// free function can do, and `framing-datagram`, whose cases are listed to
/// show the ABI refuses `datagram@1`.
pub const FILES: [&str; 6] = [
    "name-hash",
    "handshake",
    "envelope",
    "framing-datagram",
    "framing-datagram2",
    "framing-jsonrpc",
];

/// The largest request id a connection is driven to by spending ids.
pub const DRIVABLE: u64 = 1_000;

/// A `datagram@2` reply's header for protocol 0, request 0: what an
/// envelope is the tail of.
pub const REPLY_HEADER: [u8; 10] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// The vectors, compiled in rather than read at run time, so the replay
/// also runs under Miri's isolation (no file system).
pub fn vectors(feature: &str) -> Vectors {
    macro_rules! file {
        ($name:literal) => {
            (
                $name,
                include_str!(concat!("../../../conformance/vectors/", $name, ".json")),
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

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn u64_of(v: &serde_json::Value) -> u64 {
    v.as_str()
        .expect("a 64-bit integer is a decimal string")
        .parse()
        .expect("a decimal")
}

pub fn str_of(v: &serde_json::Value) -> &str {
    v.as_str().expect("a string")
}

pub fn bytes_of(v: &serde_json::Value) -> Vec<u8> {
    let text = str_of(v);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

/// A vector's params, as the host's `json@1` codec writes them.
pub fn params_of(input: &serde_json::Value) -> Result<Vec<u8>, String> {
    let format = str_of(&input["format"]);
    if format != "json@1" {
        return Err(format!("params in {format}: only json@1 is encoded here"));
    }
    let value: Value =
        serde_json::from_value(input["params"].clone()).map_err(|e| e.to_string())?;
    serde_json::to_vec(&Wire(&value)).map_err(|e| e.to_string())
}

/// `(envelope, error_id, body)` from an `envelope` / `response` input.
pub fn reply_of(input: &serde_json::Value) -> (u8, u16, Vec<u8>) {
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

/// `frame` with its request id set to `id`: the id field of a `datagram@2`
/// request, or a JSON-RPC request's `id` member.
pub fn with_request_id(
    framing: &str,
    frame: &[u8],
    protocol: u16,
    id: u64,
) -> Result<Vec<u8>, String> {
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
