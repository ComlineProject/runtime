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

use comline_runtime::handshake::comline2::{
    Exchange, Hash128, Hello, Message, Negotiated, PackageVersion, Params, Protocol, ProtocolState,
    Release,
};
use comline_runtime::contract::Side;
#[allow(unused_imports)] // Not every target that includes this file needs it.
pub use comline_runtime_conformance::hex;
use comline_runtime_conformance::{
    encode_as, Framed, Input, NegotiateIn, NegotiateResult, Reply, RequestIn, Step, Vectors, Wire,
};
use comline_runtime_ffi::{COMLINE_ENVELOPE_ERR, COMLINE_ENVELOPE_FAULT, COMLINE_ENVELOPE_OK};

/// The files with cases on the ABI: every one a `comline_connection` or a
/// free function can do, and `framing-datagram`, whose cases are listed to
/// show the ABI refuses `datagram@1`.
pub const FILES: [&str; 8] = [
    "name-hash",
    "handshake",
    "handshake-comline2",
    "envelope",
    "framing-datagram",
    "framing-datagram2",
    "framing-datagram3",
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
    const FILES_TEXT: [(&str, &str); 8] = [
        file!("name-hash"),
        file!("handshake"),
        file!("handshake-comline2"),
        file!("envelope"),
        file!("framing-datagram"),
        file!("framing-datagram2"),
        file!("framing-datagram3"),
        file!("framing-jsonrpc"),
    ];
    let (_, text) = FILES_TEXT
        .iter()
        .find(|(name, _)| *name == feature)
        .unwrap_or_else(|| panic!("no vectors for {feature}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{feature}: {e}"))
}

/// The framing a `request` or `response` input names.
pub fn framing_of(input: &Input) -> Option<&str> {
    match input {
        Input::Request(Framed::Full(i)) => Some(&i.framing),
        Input::Response(Framed::Full(i)) => Some(&i.framing),
        Input::Request(Framed::Selector { framing })
        | Input::Response(Framed::Selector { framing }) => Some(framing),
        Input::PeerRefusal(i) => Some(&i.framing),
        _ => None,
    }
}

/// A vector's params, as the host's codec writes them.
pub fn params_of(input: &RequestIn) -> Result<Vec<u8>, String> {
    encode_as(&input.format, &Wire(&input.params))
        .ok_or_else(|| format!("params in {} don't encode", input.format))
}

/// `(envelope, error_id, body)` from an `envelope` / `response` input.
pub fn reply_of(reply: &Reply) -> (u8, u16, Vec<u8>) {
    match reply {
        Reply::Ok { payload } => (COMLINE_ENVELOPE_OK, 0, payload.0.clone()),
        Reply::Err { id, body } => (COMLINE_ENVELOPE_ERR, *id, body.0.clone()),
        Reply::Fault => (COMLINE_ENVELOPE_FAULT, 0, Vec::new()),
    }
}

/// How many bytes `n` takes as a varint.
fn varint_len(n: u16) -> usize {
    match n {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        _ => 3,
    }
}

/// `frame` with its request id set to `id`: the id field of a `datagram@2`
/// or `datagram@3` request, or a JSON-RPC request's `id` member.
pub fn with_request_id(
    framing: &str,
    frame: &[u8],
    protocol: u16,
    call_id: u16,
    id: u64,
) -> Result<Vec<u8>, String> {
    match framing {
        "datagram@2" | "datagram@3" => {
            let call = if framing == "datagram@2" {
                2
            } else {
                varint_len(call_id)
            };
            let at = 1 + varint_len(protocol) + call;
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

/// A `comline2_negotiate` case, as the connection replays it: set up with
/// these, `start`, feed `first` and send what it asks for when there is one, then
/// feed the case's bytes: `reply` is what `start` then writes, and `outcome`
/// ([`outcome_of`]) what the connection concluded.
pub struct Negotiation {
    pub side: u8,
    pub scope: u8,
    pub newer_consumer: u8,
    pub same_version_differs: u8,
    pub exchange: u8,
    pub local: Vec<u8>,
    pub first: Option<Vec<u8>>,
    pub reply: Vec<u8>,
    pub outcome: String,
}

/// A negotiation step's case on a connection. Each step is what a connection does
/// with what it reads next: `answer_offer` on an `offer` responder,
/// `conclude_answer` on an `offer` initiator, `judge_hello` on a `symmetric` end.
/// `finish` needs what the end concluded before (`pending`), which only a
/// handshake gives a connection, so it's built from a peer's hello that leads
/// there ([`leading_to`]).
pub fn negotiation_of(input: &NegotiateIn) -> Result<Negotiation, String> {
    let need = |what: &str| format!("a {:?} step needs {what}", input.step);
    let code = |side: Side| side.code();
    let (local, first, side, params) = match input.step {
        Step::Finish => {
            let pending = input.pending.as_ref().ok_or_else(|| need("pending"))?;
            let (local, peer) = leading_to(pending)?;
            let params = Params {
                exchange: Exchange::Symmetric,
                ..Params::default()
            };
            let side = input.side.ok_or_else(|| need("side"))?;
            (local, Some(peer), side, params)
        }
        step => {
            let params = input.params.ok_or_else(|| need("params"))?;
            let local = input.local.clone().ok_or_else(|| need("local"))?;
            let side = match step {
                Step::AnswerOffer => Side::Responder,
                Step::ConcludeAnswer => Side::Initiator,
                _ => input.side.ok_or_else(|| need("side"))?,
            };
            let expected = match step {
                Step::JudgeHello => Exchange::Symmetric,
                _ => Exchange::Offer,
            };
            if params.exchange != expected {
                return Err(format!(
                    "no connection does {step:?} under {:?}",
                    params.exchange
                ));
            }
            (local, None, side, params)
        }
    };
    let reply = match &input.reply {
        Some(Some(message)) => message.to_bytes(),
        _ => Vec::new(),
    };
    Ok(Negotiation {
        side: code(side),
        scope: params.scope as u8,
        newer_consumer: params.newer_consumer as u8,
        same_version_differs: params.same_version_differs as u8,
        exchange: params.exchange as u8,
        local: Message::Hello(local).to_bytes(),
        first: first.map(|peer| Message::Hello(peer).to_bytes()),
        reply,
        outcome: outcome_of(&input.result),
    })
}

/// This end's hello and a peer's, which together conclude `pending`: the same
/// package and shapes, with the functions the peer lacks missing from its own.
fn leading_to(pending: &Negotiated) -> Result<(Hello, Hello), String> {
    let hello = |functions: &dyn Fn(&[u16]) -> Vec<u16>, version: PackageVersion| Hello {
        exchange: Exchange::Symmetric,
        package_id: Hash128([1; 16]),
        version,
        encodings: Vec::new(),
        protocols: pending
            .protocols
            .iter()
            .map(|p| {
                let ProtocolState::Accepted { peer_lacks } = &p.state else {
                    return Protocol {
                        number: p.number,
                        line: Release::new(0, 0, 0),
                        shape_hash: Hash128([2; 16]),
                        functions: Vec::new(),
                    };
                };
                Protocol {
                    number: p.number,
                    line: Release::new(0, 1, 0),
                    shape_hash: Hash128([2; 16]),
                    functions: functions(peer_lacks),
                }
            })
            .collect(),
    };
    if pending
        .protocols
        .iter()
        .any(|p| matches!(p.state, ProtocolState::Refused { .. }))
    {
        return Err("a pending conclusion with a refused protocol".into());
    }
    // A function both have, then those only this end has.
    let local = hello(
        &|lacks| {
            let mut all = lacks.to_vec();
            all.push(u16::MAX);
            all
        },
        pending.peer_version.clone(),
    );
    let peer = hello(&|_| vec![u16::MAX], pending.peer_version.clone());
    Ok((local, peer))
}

/// What a negotiation came to, as one field of an export's line:
///
/// - `refused/REASON/SIDE/PROTOCOL`, `PROTOCOL` `-` when it's about no protocol;
/// - `accepted/MAJOR/MINOR/PATCH/PRE/PROTOCOLS/WARNINGS`, `PRE` `-` for a release,
///   `PROTOCOLS` `NUMBER:a:LACKS` or `NUMBER:r:REASON`, comma-separated, with
///   `LACKS` and `WARNINGS` numbers joined by `+`; an empty list is `-`.
pub fn outcome_of(result: &NegotiateResult) -> String {
    match result {
        NegotiateResult::Refused(r) => refused_text(r.reason.code(), r.side.code(), r.protocol),
        NegotiateResult::Accepted(n) => {
            let protocols: Vec<String> = n
                .protocols
                .iter()
                .map(|p| match &p.state {
                    ProtocolState::Accepted { peer_lacks } => {
                        format!("{}:a:{}", p.number, joined(peer_lacks))
                    }
                    ProtocolState::Refused { reason } => {
                        format!("{}:r:{}", p.number, reason.code())
                    }
                })
                .collect();
            let release = n.peer_version.release;
            accepted_text(
                (release.major, release.minor, release.patch),
                &n.peer_version.pre,
                &protocols,
                &n.warnings,
            )
        }
    }
}

/// [`outcome_of`]'s `refused/...`.
pub fn refused_text(reason: u8, side: u8, protocol: Option<u16>) -> String {
    match protocol {
        Some(p) => format!("refused/{reason}/{side}/{p}"),
        None => format!("refused/{reason}/{side}/-"),
    }
}

/// [`outcome_of`]'s `accepted/...`.
pub fn accepted_text(
    release: (u32, u32, u32),
    pre: &str,
    protocols: &[String],
    warnings: &[u16],
) -> String {
    format!(
        "accepted/{}/{}/{}/{}/{}/{}",
        release.0,
        release.1,
        release.2,
        if pre.is_empty() { "-" } else { pre },
        if protocols.is_empty() {
            "-".to_string()
        } else {
            protocols.join(",")
        },
        joined(warnings),
    )
}

/// Numbers joined by `+`, `-` when there are none.
pub fn joined(numbers: &[u16]) -> String {
    if numbers.is_empty() {
        return "-".to_string();
    }
    numbers
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join("+")
}
