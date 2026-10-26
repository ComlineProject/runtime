// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `export_vectors`: the conformance cases the ABI carries, written out for
//! hosts that can't read the vectors' JSON or run a codec, so the C harness
//! (`tests/c/vectors.c`) and the Node harness (`js/test/vectors.test.mjs`)
//! replay exactly what `tests/vectors.rs` replays, with the same checks.
//!
//! ```text
//! cargo run -p comline-runtime-ffi --example export_vectors -- <dir>
//! ```
//!
//! It writes one file per vector file, `<dir>/<feature>.txt`, for each of
//! `name-hash`, `handshake`, `envelope`, `framing-datagram`,
//! `framing-datagram2` and `framing-jsonrpc`.
//!
//! # The format
//!
//! Plain ASCII text, one record per line, `\n`-terminated, fields separated
//! by one space, no line over [`MAX_LINE`] bytes, so a C11 program reads it
//! with `fgets` and splits it on spaces. The first line is the header:
//!
//! ```text
//! comline-ffi-vectors 1 <feature> <count>
//! ```
//!
//! `1` is the format's version, and `<count>` is the number of case lines
//! that follow: every case in the vector file, one line each, in the file's
//! order. A harness fails when it replays any other number.
//!
//! A case line is `<op> <id> <how>` and the op's fields. A field is a
//! decimal integer, a name (an encoding name, `[A-Za-z0-9@._-]`, as it is),
//! or bytes in lowercase hex, `-` when empty. Text that may hold anything
//! (a name to hash, a method) is UTF-8 in hex.
//!
//! | Line | The harness does |
//! |---|---|
//! | `name_hash ID exact NAME HASH` | `comline_name_hash(NAME)`, 8 bytes little-endian, is `HASH`. |
//! | `handshake ID exact IR_HASH WIRE FRAMING CAPS BYTES ENCODED` | `comline_handshake_encode` gives `ENCODED` (and 31 for 30 bytes of room); `comline_handshake_check` of `BYTES` gives the input's hashes and bits. When `comline_supports(FRAMING)`, a `comline@1` connection's `start` writes `ENCODED` and `feed(BYTES)` is `HANDSHAKE_OK` with `CAPS`. |
//! | `handshake ID reject BYTES` | `check` against a valid frame is `ERR_HANDSHAKE` and leaves `*peer_out` alone; a `comline@1` `datagram@2` connection's `feed` is `ERR_HANDSHAKE`, then `ERR_STATE`. |
//! | `envelope ID exact ENVELOPE ERROR_ID BODY FRAME ENCODED` | On `datagram@2`, `encode_reply(0, 0, …)` gives `ENCODED`; a connection waiting on request 0 is fed `FRAME` and reads that reply. |
//! | `envelope ID reject FRAME` | Fed `FRAME`, a connection waiting on request 0 gives `ERR_FRAMING`. |
//! | `request ID exact\|patched FRAMING PROTOCOL CALL_ID METHOD ONE_WAY REQUEST_ID DRIVE PARAMS BYTES ENCODED` | A connection spends one-way ids up to `DRIVE`, then `encode_request` gives `ENCODED` (at id `DRIVE` unless one-way) and waits unless one-way; a fresh one fed `BYTES` reads the request, `REQUEST_ID`, `PARAMS` and the call. |
//! | `request ID reject FRAMING BYTES` | `feed(BYTES)` is `ERR_FRAMING`, or an event that isn't a request. |
//! | `response ID exact FRAMING PROTOCOL REQUEST_ID ENVELOPE ERROR_ID BODY BYTES ENCODED` | `encode_reply` gives `ENCODED`; a connection that spent ids up to `REQUEST_ID` and called is fed `BYTES`, reads that reply, and waits no more. |
//! | `response ID reject FRAMING BYTES` | `feed(BYTES)` is `ERR_FRAMING`, or an event that is neither a reply nor stale. |
//! | `unmapped ID refused FRAMING` | No ABI operation does it (`datagram@1`): `comline_supports(FRAMING)` is 0 and `comline_connection_new` is `ERR_UNKNOWN_FRAMING`. |
//!
//! `ENVELOPE` is a `COMLINE_ENVELOPE_*` value; `ONE_WAY` is 0 or 1;
//! `PARAMS` and `BODY` are the host codec's bytes (params are pre-encoded
//! here with the `json@1` codec, the host's side of the boundary, so neither
//! harness needs one).
//!
//! # What `ENCODED` is
//!
//! The bytes the ABI must write, compared byte for byte, which is never
//! weaker than `tests/vectors.rs`:
//!
//! - A `"compare": "bytes"` case's own bytes, as they are (`BYTES`, or
//!   `FRAME` for an envelope).
//! - A `"compare": "json"` case's bytes as `comline-runtime`, the reference
//!   the conformance harness holds to these vectors, writes them; checked
//!   here to be JSON-equal to the case's. The ABI wraps the same encoder, so
//!   byte equality with it is what JSON equality with the case would accept,
//!   and stricter. The C and Node harnesses have no JSON parser to do it.
//! - A `patched` case's id (`u64::MAX`, too large to drive a connection to)
//!   can't be the connection's, so `DRIVE` is 0 and `ENCODED` is the
//!   reference's frame at id 0. Here, that frame with its id set back to
//!   `REQUEST_ID` is checked against the case's bytes, as `tests/vectors.rs`
//!   checks the ABI's frame. Decoding is exact either way.
//!
//! The mapping rules themselves (which file, the params, the id rules) are
//! `tests/common/cases.rs`, which `tests/vectors.rs` uses too. Nothing here
//! calls the ABI: the expectations come from the vectors and the reference
//! alone.

#[path = "../tests/common/cases.rs"]
mod cases;

use std::fmt::Write as _;
use std::path::Path;

use comline_runtime::contract::{ConnectionFraming, Datagram2Framing, Envelope};
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime_conformance::{json_equal, Case, Compare, Expect, Op};
use comline_runtime_ffi::{COMLINE_ENVELOPE_ERR, COMLINE_ENVELOPE_FAULT, COMLINE_ENVELOPE_OK};

use cases::*;

/// The longest line, newline included; the harnesses' line buffers are
/// twice it.
const MAX_LINE: usize = 4096;

/// The format's version, in the header.
const VERSION: u32 = 1;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| fail("usage: export_vectors <dir>"));
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
    for feature in FILES {
        let vectors = vectors(feature);
        let mut text = format!(
            "comline-ffi-vectors {VERSION} {feature} {}\n",
            vectors.cases.len()
        );
        for case in &vectors.cases {
            let line = line(case).unwrap_or_else(|why| fail(&format!("{}: {why}", case.id)));
            assert!(line.len() < MAX_LINE, "{}: a line over {MAX_LINE}", case.id);
            text.push_str(&line);
            text.push('\n');
        }
        let path = dir.join(format!("{feature}.txt"));
        std::fs::write(&path, text).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
        println!("{}: {} cases", path.display(), vectors.cases.len());
    }
}

fn fail(why: &str) -> ! {
    eprintln!("export_vectors: {why}");
    std::process::exit(1)
}

/// Bytes as a field: lowercase hex, `-` when empty.
fn bytes(b: &[u8]) -> String {
    if b.is_empty() {
        "-".into()
    } else {
        hex(b)
    }
}

/// An encoding name as a field, as it is.
fn name(text: &str) -> Result<&str, String> {
    let plain = !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b));
    if plain {
        Ok(text)
    } else {
        Err(format!("{text:?} isn't a plain name"))
    }
}

/// The ABI's line for one case.
fn line(case: &Case) -> Result<String, String> {
    if case
        .id
        .bytes()
        .any(|b| b.is_ascii_whitespace() || !b.is_ascii_graphic())
    {
        return Err("an id with a space in it".into());
    }
    let input = &case.input;
    let framing = input["framing"].as_str();
    let reject = case.expect == Expect::Reject;
    let mut out = String::new();
    let id = &case.id;
    match (case.op, framing) {
        (Op::NameHash, _) => {
            exact_only(case)?;
            write!(
                out,
                "name_hash {id} exact {} {}",
                bytes(str_of(&input["name"]).as_bytes()),
                bytes(&case.bytes.0)
            )
        }
        (Op::Handshake, _) if reject => {
            write!(out, "handshake {id} reject {}", bytes(&case.bytes.0))
        }
        (Op::Handshake, _) => {
            exact_only(case)?;
            write!(
                out,
                "handshake {id} exact {} {} {} {} {} {}",
                u64_of(&input["ir_hash"]),
                name(str_of(&input["wire_format"]))?,
                name(str_of(&input["framing"]))?,
                input["capabilities"].as_u64().ok_or("capabilities")? as u32,
                bytes(&case.bytes.0),
                bytes(&case.bytes.0),
            )
        }
        (_, Some("datagram@1")) => write!(out, "unmapped {id} refused datagram@1"),
        (Op::Envelope, _) => {
            let mut frame = REPLY_HEADER.to_vec();
            frame.extend_from_slice(&case.bytes.0);
            if reject {
                write!(out, "envelope {id} reject {}", bytes(&frame))
            } else {
                exact_only(case)?;
                let (envelope, error_id, body) = reply_of(input);
                write!(
                    out,
                    "envelope {id} exact {envelope} {error_id} {} {} {}",
                    bytes(&body),
                    bytes(&frame),
                    bytes(&frame),
                )
            }
        }
        (Op::Request | Op::Response, Some(f @ ("datagram@2" | "jsonrpc@1"))) if reject => {
            let op = if case.op == Op::Request {
                "request"
            } else {
                "response"
            };
            write!(out, "{op} {id} reject {f} {}", bytes(&case.bytes.0))
        }
        (Op::Request, Some(f @ ("datagram@2" | "jsonrpc@1"))) => {
            let request_id = u64_of(&input["request_id"]);
            let protocol = input["protocol"].as_u64().unwrap_or(0) as u16;
            let call_id = input["call_id"].as_u64().ok_or("call_id")? as u16;
            let method = str_of(&input["method"]);
            let one_way = input["one_way"].as_bool().unwrap_or(false);
            let params = params_of(input)?;
            let patched = request_id > DRIVABLE;
            let drive = if patched || one_way { 0 } else { request_id };
            let encoded = if case.compare == Compare::Bytes && !patched {
                case.bytes.0.clone()
            } else {
                let mut frame = Vec::new();
                with_framing(f, |framing| {
                    framing.encode_request_body(
                        protocol, call_id, method, drive, one_way, &params, &mut frame,
                    )
                })?;
                frame
            };
            let compared = if patched {
                with_request_id(f, &encoded, protocol, request_id)?
            } else {
                encoded.clone()
            };
            check_same(case, &compared)?;
            write!(
                out,
                "request {id} {} {f} {protocol} {call_id} {} {} {request_id} {drive} {} {} {}",
                if patched { "patched" } else { "exact" },
                bytes(method.as_bytes()),
                u8::from(one_way),
                bytes(&params),
                bytes(&case.bytes.0),
                bytes(&encoded),
            )
        }
        (Op::Response, Some(f @ ("datagram@2" | "jsonrpc@1"))) => {
            let request_id = u64_of(&input["request_id"]);
            let protocol = input["protocol"].as_u64().unwrap_or(0) as u16;
            let (envelope, error_id, body) = reply_of(input);
            if request_id > DRIVABLE {
                return Err("a reply id too large to wait on".into());
            }
            let encoded = if case.compare == Compare::Bytes {
                case.bytes.0.clone()
            } else {
                let reply = match envelope {
                    COMLINE_ENVELOPE_OK => Envelope::Ok(&body),
                    COMLINE_ENVELOPE_ERR => Envelope::Err {
                        id: error_id,
                        body: &body,
                    },
                    COMLINE_ENVELOPE_FAULT => Envelope::Fault,
                    _ => unreachable!(),
                };
                let mut frame = Vec::new();
                with_framing(f, |framing| {
                    framing.encode_reply(protocol, request_id, &reply, &mut frame)
                })?;
                frame
            };
            check_same(case, &encoded)?;
            write!(
                out,
                "response {id} exact {f} {protocol} {request_id} {envelope} {error_id} {} {} {}",
                bytes(&body),
                bytes(&case.bytes.0),
                bytes(&encoded),
            )
        }
        _ => return Err(format!("no mapping for {:?} on {framing:?}", case.op)),
    }
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Only `"compare": "bytes"` makes sense for this op; a JSON comparison
/// would need a reference encoding nobody has written for it.
fn exact_only(case: &Case) -> Result<(), String> {
    if case.compare == Compare::Bytes {
        Ok(())
    } else {
        Err(format!(
            "{:?} with \"compare\": \"json\" isn't exported",
            case.op
        ))
    }
}

/// `encoded` is what the case's bytes hold, as `tests/vectors.rs` compares.
fn check_same(case: &Case, encoded: &[u8]) -> Result<(), String> {
    let same = encoded == case.bytes.0.as_slice()
        || (case.compare == Compare::Json && json_equal(encoded, &case.bytes.0));
    if same {
        Ok(())
    } else {
        Err(format!("the reference writes {}", hex(encoded)))
    }
}

/// Run `f` with the reference's framing by name.
fn with_framing(name: &str, f: impl FnOnce(&dyn Reference)) -> Result<(), String> {
    match name {
        "datagram@2" => f(&Datagram2Framing),
        "jsonrpc@1" => f(&JsonRpcFraming::default()),
        other => return Err(format!("no reference for {other}")),
    }
    Ok(())
}

/// The two `ConnectionFraming` calls the export needs, object-safe.
trait Reference {
    #[allow(clippy::too_many_arguments)]
    fn encode_request_body(
        &self,
        protocol: u16,
        call_id: u16,
        name: &str,
        request_id: u64,
        one_way: bool,
        params: &[u8],
        out: &mut Vec<u8>,
    );
    fn encode_reply(&self, protocol: u16, request_id: u64, reply: &Envelope<'_>, out: &mut Vec<u8>);
}

impl<F: ConnectionFraming> Reference for F {
    fn encode_request_body(
        &self,
        protocol: u16,
        call_id: u16,
        name: &str,
        request_id: u64,
        one_way: bool,
        params: &[u8],
        out: &mut Vec<u8>,
    ) {
        ConnectionFraming::encode_request_body(
            self, protocol, call_id, name, request_id, one_way, params, out,
        )
    }

    fn encode_reply(
        &self,
        protocol: u16,
        request_id: u64,
        reply: &Envelope<'_>,
        out: &mut Vec<u8>,
    ) {
        ConnectionFraming::encode_reply(self, protocol, request_id, reply, out)
    }
}
