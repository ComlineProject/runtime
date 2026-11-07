// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline-runtime` answering a case's questions: what an input encodes to,
//! and what bytes decode to.

use serde::Serialize;

use comline_runtime::contract::stream::{self, Split, PREFIX_LEN};
use comline_runtime::contract::SliceBuf;
use comline_runtime::contract::{
    name_hash, Call, ConnectionFraming, Datagram2Framing, Datagram3Framing, DatagramFraming,
    Envelope, Frame, Framing, HandshakeFrame, RequestCall, WireFormat, FORMAT_JSON, FORMAT_MSGPACK,
    FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2, FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};
use comline_runtime::format::{Json, MsgPack, Packed};
use comline_runtime::framing::JsonRpcFraming;

use crate::runner::{judge, questions};
use crate::profile::{official_output, official_payload};
use crate::{
    comline2, hex, intern, json_equal, seed, Case, Compare, DecodeIn, Expect, Framed, Hex, Input, Op,
    Outcome, Reply, RequestIn, ResponseIn, StreamPrefixIn, StreamSplit, StreamSplitIn, Type, TypeSeed,
    Value, Wire,
};

/// `value` in `format`: `None` for a format this doesn't ship, or a value the
/// format refuses.
pub fn encode_as(format: &str, value: &(impl Serialize + ?Sized)) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match format {
        FORMAT_JSON => Json.encode(value, &mut out),
        FORMAT_MSGPACK => MsgPack.encode(value, &mut out),
        FORMAT_PACKED => Packed.encode(value, &mut out),
        _ => return None,
    }
    .ok()?;
    Some(out)
}

pub(crate) fn encode_with(format: &str, value: &Value) -> Option<Vec<u8>> {
    encode_as(format, &Wire(value))
}

/// Runs `$body` with `$f` as the format `$name` names, among those a
/// request's params are written in here.
macro_rules! with_params_format {
    ($name:expr, |$f:ident| $body:expr) => {
        match $name {
            FORMAT_JSON => {
                let $f = Json;
                $body
            }
            FORMAT_MSGPACK => {
                let $f = MsgPack;
                $body
            }
            FORMAT_PACKED => {
                let $f = Packed;
                $body
            }
            other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
        }
    };
}

fn framing_of(name: &str) -> Option<AnyFraming> {
    match name {
        FRAMING_DATAGRAM => Some(AnyFraming::Datagram(DatagramFraming)),
        FRAMING_JSONRPC => Some(AnyFraming::JsonRpc(JsonRpcFraming)),
        _ => None,
    }
}

enum AnyFraming {
    Datagram(DatagramFraming),
    JsonRpc(JsonRpcFraming),
}

macro_rules! with_framing {
    ($f:expr, |$x:ident| $body:expr) => {
        match $f {
            AnyFraming::Datagram($x) => $body,
            AnyFraming::JsonRpc($x) => $body,
        }
    };
}

#[derive(Clone, Copy)]
enum AnyDatagram {
    D2(Datagram2Framing),
    D3(Datagram3Framing),
}

fn datagram_of(name: &str) -> Option<AnyDatagram> {
    match name {
        FRAMING_DATAGRAM2 => Some(AnyDatagram::D2(Datagram2Framing)),
        FRAMING_DATAGRAM3 => Some(AnyDatagram::D3(Datagram3Framing)),
        _ => None,
    }
}

macro_rules! with_datagram {
    ($f:expr, |$x:ident| $body:expr) => {
        match $f {
            AnyDatagram::D2($x) => $body,
            AnyDatagram::D3($x) => $body,
        }
    };
}

fn encode_reply(reply: &Reply, out: &mut Vec<u8>) {
    match reply {
        Reply::Ok { payload } => Envelope::encode_ok(&payload.0, out),
        Reply::Err { id, body } => Envelope::encode_err(*id, &body.0, out),
        Reply::Fault => Envelope::encode_fault(out),
    }
}

fn envelope_of(reply: &Reply) -> Envelope<'_> {
    match reply {
        Reply::Ok { payload } => Envelope::Ok(&payload.0),
        Reply::Err { id, body } => Envelope::Err {
            id: *id,
            body: &body.0,
        },
        Reply::Fault => Envelope::Fault,
    }
}

fn reply_of(envelope: Envelope) -> Reply {
    match envelope {
        Envelope::Ok(payload) => Reply::Ok {
            payload: Hex(payload.to_vec()),
        },
        Envelope::Err { id, body } => Reply::Err {
            id,
            body: Hex(body.to_vec()),
        },
        Envelope::Fault => Reply::Fault,
    }
}

/// A request's frame as a caller that serialises the params itself makes
/// it: the params with the format alone, then `encode_request_body`, once
/// per trait the framing frames requests through. Each must equal what
/// [`encode`] writes. The method name is borrowed, not interned, as a
/// binding's is.
fn encode_request_bodies(i: &RequestIn) -> Result<Vec<(&'static str, Vec<u8>)>, Outcome> {
    let mut params = Vec::new();
    with_params_format!(i.format.as_str(), |f| f.encode(&Wire(&i.params), &mut params))
    .map_err(|e| Outcome::Fail(format!("encoding the params failed: {e:?}")))?;
    let (call, name, id) = (i.call_id, i.method.as_str(), i.request_id);
    let framed = |encode: &dyn Fn(&mut Vec<u8>)| {
        let mut out = Vec::new();
        encode(&mut out);
        out
    };
    let mut bodies = Vec::new();
    match i.framing.as_str() {
        FRAMING_DATAGRAM => bodies.push((
            "Framing::encode_request_body",
            framed(&|out| DatagramFraming.encode_request_body(call, name, id, &params, out)),
        )),
        FRAMING_DATAGRAM2 | FRAMING_DATAGRAM3 => {
            let protocol = i
                .protocol
                .ok_or_else(|| Outcome::Fail("no `protocol`".into()))?;
            let dg = datagram_of(&i.framing).unwrap();
            bodies.push((
                "ConnectionFraming::encode_request_body",
                framed(&|out| {
                    with_datagram!(dg, |f| f
                        .encode_request_body(protocol, call, name, id, i.one_way, &params, out))
                }),
            ));
        }
        FRAMING_JSONRPC => {
            if !i.one_way {
                bodies.push((
                    "Framing::encode_request_body",
                    framed(&|out| {
                        Framing::encode_request_body(&JsonRpcFraming, call, name, id, &params, out)
                    }),
                ));
            }
            bodies.push((
                "ConnectionFraming::encode_request_body",
                framed(&|out| {
                    ConnectionFraming::encode_request_body(
                        &JsonRpcFraming,
                        0,
                        call,
                        name,
                        id,
                        i.one_way,
                        &params,
                        out,
                    )
                }),
            ));
        }
        other => return Err(Outcome::NotImplemented(format!("framing `{other}`"))),
    }
    Ok(bodies)
}

/// Refuses a request its framing can't carry, by the facts of the official
/// profiles. An encoding without one is left to what comes next, which says
/// it isn't implemented.
fn fits(i: &RequestIn) -> Result<(), Outcome> {
    let (Some(payload), Some(output)) = (official_payload(&i.framing), official_output(&i.format))
    else {
        return Ok(());
    };
    let carried = payload.carries(output)
        && encode_with(&i.format, &i.params).is_none_or(|params| payload.carries_params(&params));
    if carried {
        return Ok(());
    }
    Err(Outcome::NotImplemented(format!(
        "these params in `{}` on `{}`: it takes `{}`, and they're `{}`",
        i.format,
        i.framing,
        payload.as_str(),
        output.as_str(),
    )))
}

/// Encodes `input` for `op` with `comline-runtime`.
pub(crate) fn encode(input: &Input) -> Result<Vec<u8>, Outcome> {
    if let Input::Request(Framed::Full(i)) = input {
        fits(i)?;
    }
    let mut out = Vec::new();
    match input {
        Input::NameHash(i) => {
            out.extend_from_slice(&name_hash(&i.name).to_le_bytes());
        }
        Input::Handshake(Some(i)) => {
            HandshakeFrame::new(i.ir_hash, &i.wire_format, &i.framing, i.capabilities)
                .encode(&mut out);
        }
        Input::Envelope(Some(reply)) => encode_reply(reply, &mut out),
        Input::Request(Framed::Full(i)) => {
            if let Some(dg) = datagram_of(&i.framing) {
                let protocol = i
                    .protocol
                    .ok_or_else(|| Outcome::Fail("no `protocol`".into()))?;
                let call = Call::new(i.call_id, intern::name(&i.method));
                let encoded = with_params_format!(i.format.as_str(), |format| with_datagram!(
                    dg,
                    |f| f.encode_request(
                        protocol,
                        call,
                        i.request_id,
                        i.one_way,
                        &Wire(&i.params),
                        &format,
                        &mut out,
                    )
                ));
                encoded.map_err(|e| Outcome::Fail(format!("encoding failed: {e:?}")))?;
                return Ok(out);
            }
            if i.one_way && i.framing == FRAMING_JSONRPC {
                // Only a connection sends one-way calls as notifications.
                let call = Call::new(i.call_id, intern::name(&i.method));
                let f = JsonRpcFraming;
                let encoded = match i.format.as_str() {
                    FORMAT_JSON => ConnectionFraming::encode_request(
                        &f,
                        0,
                        call,
                        i.request_id,
                        true,
                        &Wire(&i.params),
                        &Json,
                        &mut out,
                    ),
                    other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
                };
                encoded.map_err(|e| Outcome::Fail(format!("encoding failed: {e:?}")))?;
                return Ok(out);
            }
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            let call = Call::new(i.call_id, intern::name(&i.method));
            let encoded = with_params_format!(i.format.as_str(), |format| with_framing!(
                framing,
                |f| Framing::encode_request(
                    &f,
                    call,
                    i.request_id,
                    &Wire(&i.params),
                    &format,
                    &mut out
                )
            ));
            encoded.map_err(|e| Outcome::Fail(format!("encoding failed: {e:?}")))?;
        }
        Input::Response(Framed::Full(i)) => {
            if let Some(dg) = datagram_of(&i.framing) {
                let protocol = i
                    .protocol
                    .ok_or_else(|| Outcome::Fail("no `protocol`".into()))?;
                with_datagram!(dg, |f| f.encode_reply(
                    protocol,
                    i.request_id,
                    &envelope_of(&i.reply),
                    &mut out,
                ));
                return Ok(out);
            }
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            with_framing!(framing, |f| match &i.reply {
                Reply::Ok { payload } => f.encode_response_ok(i.request_id, &payload.0, &mut out),
                Reply::Err { id, body } => {
                    f.encode_response_err(i.request_id, *id, &body.0, &mut out)
                }
                Reply::Fault => f.encode_response_fault(i.request_id, &mut out),
            });
        }
        Input::Encode(i) => {
            if let Some(ty) = &i.ty {
                seed::check(&i.value, ty).map_err(Outcome::Fail)?;
                // An encoder refuses a value over its bounds before writing
                // any of it.
                seed::within_bounds(&i.value, ty)
                    .map_err(|b| Outcome::Fail(format!("refused to encode: {b}")))?;
            }
            return encode_with(&i.format, &i.value)
                .ok_or_else(|| Outcome::NotImplemented(format!("format `{}`", i.format)));
        }
        Input::Decode(_) => return Err(Outcome::Fail("`decode` cases are decode only".into())),
        Input::Comline2Message(Some(message)) => return Ok(message.to_bytes()),
        Input::Comline2Negotiate(_) => {
            return Err(Outcome::Fail("`comline2_negotiate` cases are decode only".into()))
        }
        Input::HandshakeCheck(_) => {
            return Err(Outcome::Fail(
                "`handshake_check` cases are decode only".into(),
            ))
        }
        Input::StreamPrefix(i) => {
            let len = usize::try_from(i.len)
                .map_err(|_| Outcome::Fail(format!("refused: a {}-byte frame", i.len)))?;
            let mut prefix = [0u8; PREFIX_LEN];
            stream::put_prefix(len, stream_max(i.max), &mut SliceBuf::new(&mut prefix))
                .map_err(|e| Outcome::Fail(format!("refused: {e}")))?;
            out.extend_from_slice(&prefix);
        }
        Input::StreamSplit(StreamSplit::Frames(i)) => {
            for frame in &i.frames {
                let mut prefix = [0u8; PREFIX_LEN];
                stream::put_prefix(
                    frame.0.len(),
                    stream_max(i.max),
                    &mut SliceBuf::new(&mut prefix),
                )
                .map_err(|e| Outcome::Fail(format!("refused: {e}")))?;
                out.extend_from_slice(&prefix);
                out.extend_from_slice(&frame.0);
            }
            out.extend_from_slice(&i.rest.0);
        }
        // A rejection's input selects a decoder; there's nothing to encode.
        Input::Handshake(None)
        | Input::Envelope(None)
        | Input::Request(Framed::Selector { .. })
        | Input::Response(Framed::Selector { .. })
        | Input::PeerRefusal(_)
        | Input::StreamSplit(StreamSplit::Limit { .. })
        | Input::Comline2Message(None) => {
            return Err(Outcome::Fail(format!(
                "bad input: {} is only enough to decode",
                to_json(input)
            )))
        }
    }
    Ok(out)
}

/// Decodes `bytes` for `op` with `comline-runtime`, as the input that would
/// encode to them; `Ok(None)` when the runtime refuses the bytes.
pub(crate) fn decode(input: &Input, bytes: &[u8]) -> Result<Option<serde_json::Value>, Outcome> {
    let json = |v: serde_json::Value| -> Result<Option<serde_json::Value>, Outcome> { Ok(Some(v)) };
    match input {
        Input::NameHash(_) | Input::Encode(_) => Ok(None),
        Input::Comline2Message(_) => Ok(comline_runtime::comline2::Message::decode(bytes)
            .ok()
            .map(|m| to_json(&m))),
        Input::Comline2Negotiate(i) => Ok(Some(to_json(&comline2::negotiate(i.clone(), bytes)?))),
        Input::StreamPrefix(i) => {
            let Ok(prefix) = <[u8; PREFIX_LEN]>::try_from(bytes) else {
                return Ok(None);
            };
            Ok(stream::frame_len(prefix, stream_max(i.max)).map(|len| {
                to_json(&StreamPrefixIn {
                    len: len as u64,
                    max: i.max,
                })
            }))
        }
        Input::StreamSplit(i) => {
            let max = i.max();
            let mut frames = Vec::new();
            let mut at = 0;
            loop {
                match stream::split_frame(&bytes[at..], stream_max(max)) {
                    Split::Frame { start, len } => {
                        frames.push(Hex(bytes[at + start..at + start + len].to_vec()));
                        at += start + len;
                    }
                    Split::NeedMore { needed } => {
                        return json(to_json(&StreamSplitIn {
                            max,
                            frames,
                            rest: Hex(bytes[at..].to_vec()),
                            needed: needed as u64,
                        }));
                    }
                    Split::TooLarge => return Ok(None),
                }
            }
        }
        Input::Decode(i) => Ok(decode_typed(&i.format, &i.ty, bytes)?.map(|value| {
            to_json(&DecodeIn {
                format: i.format.clone(),
                ty: i.ty.clone(),
                value: Some(value),
            })
        })),
        Input::Handshake(i) => {
            let Some(h) = HandshakeFrame::decode(bytes) else {
                return Ok(None);
            };
            // The frame carries hashes; compare them against the input's names.
            let named = |hash: u64, name: Option<&String>| match name {
                Some(n) if name_hash(n) == hash => n.clone(),
                _ => format!("{hash:#018x}"),
            };
            json(serde_json::json!({
                "ir_hash": h.ir_hash.to_string(),
                "wire_format": named(h.wire_format, i.as_ref().map(|i| &i.wire_format)),
                "framing": named(h.framing, i.as_ref().map(|i| &i.framing)),
                "capabilities": h.capabilities,
            }))
        }
        Input::HandshakeCheck(i) => {
            let ours = HandshakeFrame::new(i.ir_hash, &i.wire_format, &i.framing, i.capabilities);
            match ours.check_bytes(bytes) {
                Ok(_) => json(to_json(i)),
                Err(_) => Ok(None),
            }
        }
        Input::Envelope(_) => match Envelope::decode(bytes) {
            Some(e) => json(to_json(&reply_of(e))),
            None => Ok(None),
        },
        Input::Request(Framed::Selector { framing }) => {
            if let Some(dg) = datagram_of(framing) {
                let frame = with_datagram!(dg, |f| f.decode(bytes));
                return Ok(matches!(frame, Some(Frame::Request { protocol: Some(_), .. }))
                    .then(|| serde_json::json!({ "accepted": true })));
            }
            let framing = framing_of(framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{framing}`")))?;
            Ok(with_framing!(framing, |f| f.decode_request(bytes).is_some())
                .then(|| serde_json::json!({ "accepted": true })))
        }
        Input::Request(Framed::Full(i)) if datagram_of(&i.framing).is_some() => {
            let dg = datagram_of(&i.framing).unwrap();
            let Some(Frame::Request {
                protocol: Some(protocol),
                request,
                ..
            }) = with_datagram!(dg, |f| f.decode(bytes))
            else {
                return Ok(None);
            };
            let params = encode_with(&i.format, &i.params)
                .ok_or_else(|| Outcome::NotImplemented(format!("format `{}`", i.format)))?;
            let params = if request.params == params.as_slice() {
                to_json(&i.params)
            } else {
                serde_json::json!({ "undecoded": to_json(&Hex(request.params.to_vec())) })
            };
            let RequestCall::Id(call_id) = request.call else {
                return Ok(None);
            };
            json(serde_json::json!({
                "framing": i.framing,
                "protocol": protocol,
                "call_id": call_id,
                "method": i.method,
                "request_id": request.request_id.to_string(),
                "format": i.format,
                "params": params,
            }))
        }
        Input::Request(Framed::Full(i)) => {
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            // A notification is a connection's frame: read it as one, which
            // says whether the frame wanted a reply.
            let (req, one_way) = if i.one_way && i.framing == FRAMING_JSONRPC {
                match ConnectionFraming::decode(&JsonRpcFraming, bytes) {
                    Some(Frame::Request {
                        request, one_way, ..
                    }) => (request, one_way),
                    _ => return Ok(None),
                }
            } else {
                match with_framing!(framing, |f| f.decode_request(bytes)) {
                    Some(req) => (req, false),
                    None => return Ok(None),
                }
            };
            let (call_id, method) = match req.call {
                RequestCall::Id(id) => (id, i.method.clone()),
                RequestCall::Name(name) => (i.call_id, name.to_string()),
            };
            let params = encode_with(&i.format, &i.params)
                .ok_or_else(|| Outcome::NotImplemented(format!("format `{}`", i.format)))?;
            // Decoding gives params as bytes; they stand for the input's
            // value only when they're its exact encoding.
            let params = if req.params == params.as_slice() {
                to_json(&i.params)
            } else {
                serde_json::json!({ "undecoded": to_json(&Hex(req.params.to_vec())) })
            };
            let mut decoded = serde_json::json!({
                "framing": i.framing,
                "call_id": call_id,
                "method": method,
                "request_id": req.request_id.to_string(),
                "format": i.format,
                "params": params,
            });
            if one_way {
                decoded["one_way"] = true.into();
            }
            json(decoded)
        }
        Input::PeerRefusal(i) => {
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            if with_framing!(framing, |f| Framing::is_peer_refusal(&f, bytes)) {
                json(to_json(i))
            } else {
                Ok(None)
            }
        }
        Input::Response(i) => {
            let framing_name = match i {
                Framed::Full(i) => &i.framing,
                Framed::Selector { framing } => framing,
            };
            if let Some(dg) = datagram_of(framing_name) {
                return match with_datagram!(dg, |f| f.decode(bytes)) {
                    Some(Frame::Reply {
                        protocol,
                        request_id,
                        envelope,
                    }) => json(to_json(&ResponseIn {
                        framing: framing_name.clone(),
                        protocol,
                        request_id,
                        reply: reply_of(envelope),
                    })),
                    _ => Ok(None),
                };
            }
            let framing = framing_of(framing_name)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{framing_name}`")))?;
            match with_framing!(framing, |x| x.decode_response(bytes)) {
                Some((request_id, envelope)) => json(to_json(&ResponseIn {
                    framing: framing_name.clone(),
                    protocol: None,
                    request_id,
                    reply: reply_of(envelope),
                })),
                None => Ok(None),
            }
        }
    }
}

/// A stream limit as the runtime takes it: a `max` past `usize` holds every
/// frame `usize` can.
fn stream_max(max: u64) -> usize {
    usize::try_from(max).unwrap_or(usize::MAX)
}

pub(crate) fn to_json<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).expect("vector types serialize")
}

/// The reference's answer to one question: what `comline-runtime` encodes
/// an input to, or what it decodes bytes to.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The input encodes to these bytes.
    Bytes(Vec<u8>),
    /// The bytes decode, to this input (in the case's own form).
    Accept(serde_json::Value),
    /// The runtime refuses the bytes.
    Reject,
    /// The runtime has no implementation of something the question needs.
    NotImplemented(String),
    /// The question itself is malformed.
    Bad(String),
}

/// Asks `comline-runtime` one question, as a case states it: with no
/// `bytes`, what `input` encodes to for `op`; with `bytes`, whether they
/// decode, and to what.
pub fn answer(op: Op, input: &serde_json::Value, bytes: Option<&[u8]>) -> Answer {
    let input = match Input::parse(op, input) {
        Ok(input) => input,
        Err(why) => return Answer::Bad(why),
    };
    let answered = match bytes {
        None => encode(&input).map(Answer::Bytes),
        Some(bytes) => {
            decode(&input, bytes).map(|decoded| decoded.map_or(Answer::Reject, Answer::Accept))
        }
    };
    answered.unwrap_or_else(|outcome| match outcome {
        Outcome::NotImplemented(what) => Answer::NotImplemented(what),
        Outcome::Fail(why) => Answer::Bad(why),
        Outcome::Pass => unreachable!("encode and decode fail with a reason"),
    })
}

/// Runs one case against `comline-runtime`: its questions, judged as any
/// implementation's answers are, and then what only this crate can ask.
pub fn run(case: &Case) -> Outcome {
    let answers: Vec<Answer> = questions(case)
        .iter()
        .map(|q| answer(q.op, &q.input, q.bytes.as_deref()))
        .collect();
    match judge(case, &answers) {
        Outcome::Pass => request_bodies_match(case).err().unwrap_or(Outcome::Pass),
        failed => failed,
    }
}

/// A request's frame comes out the same however it's written: a case's input
/// through each path a caller that serialises the params itself takes.
fn request_bodies_match(case: &Case) -> Result<(), Outcome> {
    let Input::Request(Framed::Full(i)) = &case.input else {
        return Ok(());
    };
    if case.expect != Expect::Exact {
        return Ok(());
    }
    let want = &case.bytes.0;
    for (path, encoded) in encode_request_bodies(i)? {
        if encoded != *want && !(case.compare == Compare::Json && json_equal(&encoded, want)) {
            return Err(Outcome::Fail(format!(
                "{path} encodes to {}, expected {}",
                hex(&encoded),
                hex(want),
            )));
        }
    }
    Ok(())
}

/// Decodes `bytes` as `ty` the way a generated type with that shape would,
/// through [`TypeSeed`]; `Ok(None)` when the format refuses them.
pub fn decode_typed(format: &str, ty: &Type, bytes: &[u8]) -> Result<Option<Value>, Outcome> {
    let seed = TypeSeed(ty);
    Ok(match format {
        FORMAT_JSON => Json.decode_seed(seed, bytes).ok(),
        FORMAT_MSGPACK => MsgPack.decode_seed(seed, bytes).ok(),
        FORMAT_PACKED => Packed.decode_seed(seed, bytes).ok(),
        other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn case(op: Op, input: serde_json::Value, bytes: &[u8], expect: Expect) -> Case {
        Case {
            id: "x/y".into(),
            input: Input::parse(op, &input).unwrap(),
            bytes: Hex(bytes.to_vec()),
            expect,
            compare: Compare::Bytes,
        }
    }

    #[test]
    fn run_judges_the_reference_as_any_implementation() {
        let name = json!({ "name": "" });
        let hash = comline_runtime::contract::name_hash("").to_le_bytes();
        assert_eq!(run(&case(Op::NameHash, name.clone(), &hash, Expect::Exact)), Outcome::Pass);
        assert!(matches!(
            run(&case(Op::NameHash, name, &[0], Expect::Exact)),
            Outcome::Fail(why) if why.contains("encodes wrong")
        ));
        // Bytes that decode, under a case that says they mustn't.
        let reply = json!({ "kind": "fault" });
        assert!(matches!(
            run(&case(Op::Envelope, reply, &[2], Expect::Reject)),
            Outcome::Fail(why) if why.contains("accepted bytes")
        ));
        // JSON-RPC's params are JSON, whatever the format says.
        let binary = json!({
            "framing": "jsonrpc@1", "call_id": 1, "method": "m", "request_id": "1",
            "format": "msgpack@1", "params": { "u8": 1 },
        });
        assert!(matches!(
            run(&case(Op::Request, binary, &[], Expect::Exact)),
            Outcome::NotImplemented(_)
        ));
        // A format it doesn't ship isn't a pass, even for a rejection.
        let value = json!({ "format": "nope@1", "value": { "u8": 1 } });
        assert!(matches!(
            run(&case(Op::Encode, value, &[], Expect::Reject)),
            Outcome::NotImplemented(_)
        ));
    }
}
