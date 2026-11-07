// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The case format: a case's operation and its input, typed by op.
//!
//! The files and the ops protocol hold an input as JSON; [`Input::parse`] is
//! the one place it's read, so nothing past it looks fields up by name.

use std::fmt::Write as _;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as Json;

use comline_runtime::comline2::Message;

use crate::comline2::NegotiateIn;
use crate::{decimal, Type, Value};

/// Bytes as lowercase hex, in vectors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hex(pub Vec<u8>);

/// `bytes` as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl Serialize for Hex {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        fn digit(b: u8) -> Option<u8> {
            match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            }
        }
        let s = String::deserialize(d)?;
        let s = s.as_bytes();
        if s.len() % 2 != 0 {
            return Err(D::Error::custom("odd-length hex"));
        }
        s.as_chunks::<2>()
            .0
            .iter()
            .map(|&[high, low]| Some(digit(high)? << 4 | digit(low)?))
            .collect::<Option<_>>()
            .map(Hex)
            .ok_or_else(|| D::Error::custom("not lowercase hex"))
    }
}

/// One vector: an operation's input and the exact bytes it must produce and
/// decode from, or (`expect: "reject"`) bytes the operation must refuse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CaseWire", into = "CaseWire")]
pub struct Case {
    pub id: String,
    /// The operation and its input; for a rejection, only what selects the
    /// decoder (a framing's name).
    pub input: Input,
    pub bytes: Hex,
    pub expect: Expect,
    pub compare: Compare,
}

impl Case {
    pub fn op(&self) -> Op {
        self.input.op()
    }
}

/// A case as a file holds it.
#[derive(Serialize, Deserialize)]
struct CaseWire {
    id: String,
    op: Op,
    #[serde(default, skip_serializing_if = "Json::is_null")]
    input: Json,
    bytes: Hex,
    #[serde(default, skip_serializing_if = "Expect::is_exact")]
    expect: Expect,
    #[serde(default, skip_serializing_if = "Compare::is_bytes")]
    compare: Compare,
}

impl TryFrom<CaseWire> for Case {
    type Error = String;

    fn try_from(case: CaseWire) -> Result<Self, String> {
        let input =
            Input::parse(case.op, &case.input).map_err(|e| format!("case `{}`: {e}", case.id))?;
        Ok(Case {
            id: case.id,
            input,
            bytes: case.bytes,
            expect: case.expect,
            compare: case.compare,
        })
    }
}

impl From<Case> for CaseWire {
    fn from(case: Case) -> Self {
        CaseWire {
            id: case.id,
            op: case.input.op(),
            input: crate::to_json(&case.input),
            bytes: case.bytes,
            expect: case.expect,
            compare: case.compare,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// `{name}` → the hash, 8 bytes little-endian. Encode only.
    NameHash,
    /// `{ir_hash, wire_format, framing, capabilities}`, names unhashed.
    Handshake,
    /// `{ir_hash, wire_format, framing, capabilities}`, one end's own
    /// declaration (`comline@1`'s), and `bytes`, the frame its peer sent.
    /// Decoding checks the peer's frame against the input: it's accepted,
    /// and gives the input back, when the schema fingerprint, format and
    /// framing agree (capability bits may differ), and refused otherwise.
    /// Decode only. A handshake profile's facts are about this.
    HandshakeCheck,
    /// A [`Reply`].
    Envelope,
    /// `{framing, call_id, method, request_id, format, params}`. Decoding
    /// yields the call (the id for datagram framing, the method for
    /// JSON-RPC), the request id, and `params` encoded in `format`. A framing
    /// that carries the protocol (`datagram@2`) has a `protocol` number too.
    Request,
    /// `{framing, request_id}` and a [`Reply`], plus `protocol` for a
    /// framing that carries it.
    Response,
    /// `{format, value}` → the encoded value. Encode only.
    Encode,
    /// `{format, type, value}`: decoding `bytes` as `type` gives `value`.
    /// Decode only: the bytes were written by a different type (a field
    /// more, a variant reordered), which is what a profile fact is about.
    /// A rejection's input is `{format, type}`.
    Decode,
    /// `{len, max}` → the 4-byte length prefix of a `len`-byte frame on a
    /// byte stream, under the limit `max`. Decoding reads the prefix back.
    /// A rejection's bytes are a prefix over `max`; its input is
    /// `{len, max}` too, and encoding it must fail as well.
    StreamPrefix,
    /// `{max, frames, rest, needed}` → a byte stream: each of `frames`
    /// (hex) behind its length prefix, then `rest`, the start of a frame
    /// not yet whole. Decoding splits the stream frame by frame under the
    /// limit `max`; `needed` is how many bytes the incomplete frame in
    /// `rest` takes in all, prefix included, or 0 while its prefix is
    /// incomplete. A rejection's bytes declare a frame over `max`; its
    /// input is `{max}`.
    StreamSplit,
    /// A `comline@2` message, `{"type": "hello" | "answer" | "verdict",
    /// "message": {...}}`, as `comline_runtime::comline2::Message`
    /// serialises it. Encode and decode.
    Comline2Message,
    /// One `comline@2` negotiation step, `{step, side, params, local,
    /// pending, reply, result}`, and `bytes`, the message the peer sent.
    /// Decoding runs the step and gives the input back when its `reply` (the
    /// message this end sends, or `null`) and `result` (`{"accepted":
    /// {...}}` or `{"refused": {reason, side, protocol}}`) are what the
    /// runtime concludes. Decode only. `comline@2`'s profile facts are
    /// about this.
    Comline2Negotiate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// Encoding the input gives `bytes`, and decoding `bytes` gives the input.
    #[default]
    Exact,
    /// Decoding `bytes` fails.
    Reject,
}

impl Op {
    /// Whether a case of this op only asks to decode its bytes, never to
    /// encode its input.
    pub fn decode_only(self) -> bool {
        matches!(
            self,
            Op::Decode | Op::HandshakeCheck | Op::Comline2Negotiate
        )
    }
}

impl Expect {
    pub(crate) fn is_exact(&self) -> bool {
        *self == Expect::Exact
    }
}

/// How a runtime's output must match a case's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compare {
    /// Byte for byte.
    #[default]
    Bytes,
    /// As JSON values (RFC 8259), for bytes that are JSON text: whitespace
    /// and object member order don't matter, and numbers compare as exact
    /// decimals, so `1` equals `1.0` but `9007199254740992` doesn't equal
    /// `9007199254740993`. The same holds for JSON carried inside the frame:
    /// JSON-RPC params, results and error data. `comline-runtime`, the
    /// reference, reproduces the bytes exactly, except for a `found-*` case
    /// that spells a string another way.
    Json,
}

impl Compare {
    pub(crate) fn is_bytes(&self) -> bool {
        *self == Compare::Bytes
    }
}

/// A response's outcome, as `envelope` and `response` inputs carry it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Ok { payload: Hex },
    Err { id: u16, body: Hex },
    Fault,
}

/// A case's operation and its input. Serializes as the input alone, which
/// is how the files and the ops protocol hold it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)] // Cases are few, and held whole.
pub enum Input {
    NameHash(NameHashIn),
    /// `None` for a rejection, which only decodes.
    Handshake(Option<HandshakeIn>),
    HandshakeCheck(HandshakeIn),
    /// `None` for a rejection, which only decodes.
    Envelope(Option<Reply>),
    Request(Framed<RequestIn>),
    Response(Framed<ResponseIn>),
    Encode(EncodeIn),
    Decode(DecodeIn),
    StreamPrefix(StreamPrefixIn),
    StreamSplit(StreamSplit),
    /// `None` for a rejection, which only decodes.
    Comline2Message(Option<Message>),
    Comline2Negotiate(NegotiateIn),
}

impl Input {
    pub fn op(&self) -> Op {
        match self {
            Input::NameHash(_) => Op::NameHash,
            Input::Handshake(_) => Op::Handshake,
            Input::HandshakeCheck(_) => Op::HandshakeCheck,
            Input::Envelope(_) => Op::Envelope,
            Input::Request(_) => Op::Request,
            Input::Response(_) => Op::Response,
            Input::Encode(_) => Op::Encode,
            Input::Decode(_) => Op::Decode,
            Input::StreamPrefix(_) => Op::StreamPrefix,
            Input::StreamSplit(_) => Op::StreamSplit,
            Input::Comline2Message(_) => Op::Comline2Message,
            Input::Comline2Negotiate(_) => Op::Comline2Negotiate,
        }
    }

    /// Reads `op`'s input. What it can't hold (an unknown field, a value
    /// spelled another way) is refused rather than dropped: an input is
    /// sent to implementations and compared with what they give back.
    pub fn parse(op: Op, input: &Json) -> Result<Input, String> {
        fn read<'a, T: Deserialize<'a>>(input: &'a Json) -> Result<T, String> {
            T::deserialize(input).map_err(|e| format!("bad input: {e}"))
        }
        let parsed = match op {
            Op::NameHash => Input::NameHash(read(input)?),
            Op::Handshake => Input::Handshake(read(input)?),
            Op::HandshakeCheck => Input::HandshakeCheck(read(input)?),
            Op::Envelope => Input::Envelope(read(input)?),
            Op::Request if only_key(input, "framing") => Input::Request(read(input)?),
            Op::Request => Input::Request(Framed::Full(read(input)?)),
            Op::Response if only_key(input, "framing") => Input::Response(read(input)?),
            Op::Response => Input::Response(Framed::Full(read(input)?)),
            Op::Encode => Input::Encode(read(input)?),
            Op::Decode => Input::Decode(read(input)?),
            Op::StreamPrefix => Input::StreamPrefix(read(input)?),
            Op::StreamSplit if only_key(input, "max") => Input::StreamSplit(read(input)?),
            Op::StreamSplit => Input::StreamSplit(StreamSplit::Frames(read(input)?)),
            Op::Comline2Message => Input::Comline2Message(read(input)?),
            Op::Comline2Negotiate => Input::Comline2Negotiate(read(input)?),
        };
        if same_shape(input, &crate::to_json(&parsed)) {
            Ok(parsed)
        } else {
            Err(format!(
                "bad input: not a `{}` input: {input}",
                crate::to_json(&op).as_str().unwrap_or_default()
            ))
        }
    }

    /// Whether `answer`, a decode's answer in JSON, gives this input back.
    /// Numbers compare as what they hold, so `1` is `{"f64": 1.0}`'s value
    /// from an implementation that doesn't keep them apart.
    pub fn given_back_by(&self, answer: &Json) -> bool {
        Input::parse(self.op(), answer).is_ok_and(|given| given == *self)
    }
}

fn only_key(input: &Json, key: &str) -> bool {
    input
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key(key))
}

/// Whether `input` holds what `parsed`, the same input read and written
/// back, holds: the same members, and the same text and elements. Numbers
/// were checked by reading them into their types.
fn same_shape(input: &Json, parsed: &Json) -> bool {
    match (input, parsed) {
        (Json::Number(_), Json::Number(_)) => true,
        (Json::Array(a), Json::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_shape(a, b))
        }
        (Json::Object(a), Json::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|w| same_shape(v, w)))
        }
        _ => input == parsed,
    }
}

/// A `request` or `response` input, or, for a rejection, only the framing
/// that selects the decoder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Framed<T> {
    Full(T),
    Selector { framing: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameHashIn {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeIn {
    #[serde(with = "decimal")]
    pub ir_hash: u64,
    pub wire_format: String,
    pub framing: String,
    pub capabilities: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestIn {
    pub framing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u16>,
    pub call_id: u16,
    pub method: String,
    #[serde(with = "decimal")]
    pub request_id: u64,
    /// A one-way call, on a connection: JSON-RPC sends it as a notification,
    /// with no `id` (and decoding one gives `request_id` 0).
    #[serde(default, skip_serializing_if = "is_false")]
    pub one_way: bool,
    pub format: String,
    pub params: Value,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseIn {
    pub framing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u16>,
    #[serde(with = "decimal")]
    pub request_id: u64,
    #[serde(flatten)]
    pub reply: Reply,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamPrefixIn {
    pub len: u64,
    pub max: u64,
}

/// A `stream_split` input, or, for a rejection, only the limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamSplit {
    Frames(StreamSplitIn),
    Limit { max: u64 },
}

impl StreamSplit {
    pub fn max(&self) -> u64 {
        match self {
            StreamSplit::Frames(input) => input.max,
            StreamSplit::Limit { max } => *max,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamSplitIn {
    pub max: u64,
    pub frames: Vec<Hex>,
    pub rest: Hex,
    pub needed: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodeIn {
    pub format: String,
    /// The type the value is encoded as, when its bounds matter: encoding
    /// a value over them must fail (a rejection), and one within them gives
    /// the bytes. Without it, the value's own shape is its type.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<Type>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodeIn {
    pub format: String,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(text: &str) -> Result<Hex, serde_json::Error> {
        serde_json::from_value(json!(text))
    }

    #[test]
    fn hex_is_lowercase_pairs() {
        assert_eq!(read("00ff").unwrap(), Hex(vec![0, 255]));
        assert_eq!(read("").unwrap(), Hex(vec![]));
        // A slice of `aéb` by bytes lands inside `é`.
        for bad in ["aéb", "+a", "FF", "0", "zz"] {
            assert!(read(bad).is_err(), "{bad}");
        }
        assert_eq!(hex(&[0, 171, 255]), "00abff");
    }

    #[test]
    fn an_input_outside_its_shape_is_refused() {
        let name = json!({ "name": "x" });
        assert!(Input::parse(Op::NameHash, &name).is_ok());
        assert!(Input::parse(Op::NameHash, &json!({ "name": "x", "more": 1 })).is_err());
        assert!(Input::parse(Op::Decode, &json!({})).is_err());
        // A nested field, on a type with no `deny_unknown_fields`.
        let reply = json!({ "framing": "f", "request_id": "1", "kind": "fault", "more": 1 });
        assert!(Input::parse(Op::Response, &reply).is_err());
        // A spelling the input wouldn't write back.
        let reply = json!({ "framing": "f", "protocol": null, "request_id": "1", "kind": "fault" });
        assert!(Input::parse(Op::Response, &reply).is_err());
        let id = json!({ "framing": "f", "request_id": "01", "kind": "fault" });
        assert!(Input::parse(Op::Response, &id).is_err());
    }

    #[test]
    fn a_rejection_input_is_only_what_selects_the_decoder() {
        let selector = json!({ "framing": "f" });
        assert_eq!(
            Input::parse(Op::Request, &selector).unwrap(),
            Input::Request(Framed::Selector {
                framing: "f".into()
            })
        );
        assert_eq!(
            Input::parse(Op::StreamSplit, &json!({ "max": 2 })).unwrap(),
            Input::StreamSplit(StreamSplit::Limit { max: 2 })
        );
        assert_eq!(
            Input::parse(Op::Envelope, &Json::Null).unwrap(),
            Input::Envelope(None)
        );
        // A malformed full input isn't taken for a selector.
        assert!(Input::parse(Op::Request, &json!({ "framing": "f", "method": "m" })).is_err());
    }

    #[test]
    fn a_case_keeps_its_op_and_input_together() {
        let case =
            json!({ "id": "a/b", "op": "name_hash", "input": { "name": "x" }, "bytes": "00" });
        let parsed: Case = serde_json::from_value(case.clone()).unwrap();
        assert_eq!(parsed.op(), Op::NameHash);
        assert_eq!(serde_json::to_value(&parsed).unwrap(), case);

        let wrong = json!({ "id": "a/b", "op": "decode", "input": { "name": "x" }, "bytes": "00" });
        let error = serde_json::from_value::<Case>(wrong)
            .unwrap_err()
            .to_string();
        assert!(error.contains("case `a/b`"), "{error}");
    }

    #[test]
    fn an_answer_gives_back_the_input_by_value() {
        let input = |f64: Json| json!({ "format": "json@1", "value": { "f64": f64 } });
        let held = Input::parse(Op::Encode, &input(json!(1.0))).unwrap();
        assert!(held.given_back_by(&input(json!(1))));
        assert!(!held.given_back_by(&input(json!(2))));
        let mut more = input(json!(1.0));
        more["more"] = json!(1);
        assert!(!held.given_back_by(&more));
    }
}
