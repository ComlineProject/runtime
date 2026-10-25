// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `session` op: one side of a connection, `comline-runtime`'s sans-IO
//! [`State`], driven through a list of events. Each event gets one step in
//! the reply: the frame a call or a reply writes, what a received frame
//! means, or nothing for an abandon. A runtime in another language runs the
//! same events through its own connection and compares the steps.
//!
//! See README.md for the request and reply shapes.

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

use comline_runtime::connection::{Event, State};
use comline_runtime::contract::{
    BufMut, Call, ConnectionFraming, Datagram2Framing, Envelope, ProtocolNumber, RequestCall,
    RuntimeError, WireFormat, FRAMING_DATAGRAM2, FRAMING_JSONRPC,
};
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime_conformance::{Hex, Reply};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionIn {
    framing: String,
    ordered: bool,
    events: Vec<EventIn>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum EventIn {
    /// A local call: frame it, and wait on it unless it's one-way.
    Call(CallIn),
    /// A frame from the peer.
    Receive(Hex),
    /// Stop waiting on a request id (a timeout).
    Abandon(#[serde(with = "decimal")] u64),
    /// This side answers the peer's request: frame the reply.
    Reply(ReplyIn),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallIn {
    protocol: ProtocolNumber,
    call_id: u16,
    method: String,
    #[serde(default)]
    one_way: bool,
    /// The params, already encoded in the session's format.
    params: Hex,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyIn {
    protocol: ProtocolNumber,
    #[serde(with = "decimal")]
    request_id: u64,
    envelope: Reply,
}

mod decimal {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

/// Answers a `session` request's `input`: `Ok` with the reply's fields, or
/// `Err` with an `error` reply's message.
pub fn session(input: &Json) -> Result<Map<String, Json>, String> {
    let input = SessionIn::deserialize(input).map_err(|e| format!("bad `session` input: {e}"))?;
    let steps = match input.framing.as_str() {
        FRAMING_DATAGRAM2 => run::<Datagram2Framing>(input.ordered, &input.events),
        FRAMING_JSONRPC => run::<JsonRpcFraming>(input.ordered, &input.events),
        other => {
            let mut reply = Map::new();
            reply.insert(
                "not_implemented".into(),
                format!("framing `{other}` on a connection").into(),
            );
            return Ok(reply);
        }
    };
    let mut reply = Map::new();
    reply.insert("steps".into(), Json::Array(steps));
    Ok(reply)
}

fn run<F: ConnectionFraming>(ordered: bool, events: &[EventIn]) -> Vec<Json> {
    let framing = F::default();
    let mut state = if ordered {
        State::default()
    } else {
        State::unordered()
    };
    let mut out = Vec::new();
    events
        .iter()
        .map(|event| match event {
            EventIn::Call(c) => {
                let call = Call::new(c.call_id, intern(&c.method));
                match state.encode_request(
                    &framing,
                    c.protocol,
                    call,
                    &(),
                    &Preencoded(&c.params.0),
                    !c.one_way,
                    &mut out,
                ) {
                    Ok(request_id) => json!({
                        "frame": Hex(out.clone()),
                        "request_id": request_id.to_string(),
                    }),
                    Err(e) => json!({ "error": error_name(e) }),
                }
            }
            EventIn::Receive(frame) => match state.receive(&framing, &frame.0) {
                Ok(event) => json!({ "event": event_json(event) }),
                Err(e) => json!({ "error": error_name(e) }),
            },
            EventIn::Abandon(request_id) => {
                state.abandon(*request_id);
                json!({})
            }
            EventIn::Reply(r) => {
                let envelope = match &r.envelope {
                    Reply::Ok { payload } => Envelope::Ok(&payload.0),
                    Reply::Err { id, body } => Envelope::Err {
                        id: *id,
                        body: &body.0,
                    },
                    Reply::Fault => Envelope::Fault,
                };
                state.encode_reply(&framing, r.protocol, r.request_id, &envelope, &mut out);
                json!({ "frame": Hex(out.clone()) })
            }
        })
        .collect()
}

fn event_json(event: Event<'_>) -> Json {
    match event {
        Event::Request {
            protocol,
            request,
            one_way,
        } => {
            let mut fields = Map::new();
            if let Some(protocol) = protocol {
                fields.insert("protocol".into(), protocol.into());
            }
            match request.call {
                RequestCall::Id(id) => fields.insert("call_id".into(), id.into()),
                RequestCall::Name(name) => fields.insert("method".into(), name.to_string().into()),
            };
            fields.insert("request_id".into(), request.request_id.to_string().into());
            fields.insert("one_way".into(), one_way.into());
            fields.insert("params".into(), json!(Hex(request.params.to_vec())));
            json!({ "request": fields })
        }
        Event::Reply {
            request_id,
            envelope,
        } => {
            let envelope = match envelope {
                Envelope::Ok(payload) => Reply::Ok {
                    payload: Hex(payload.to_vec()),
                },
                Envelope::Err { id, body } => Reply::Err {
                    id,
                    body: Hex(body.to_vec()),
                },
                Envelope::Fault => Reply::Fault,
            };
            json!({ "reply": { "request_id": request_id.to_string(), "envelope": envelope } })
        }
        Event::Stale => json!("stale"),
    }
}

/// A [`RuntimeError`]'s name in a step's `error`.
fn error_name(error: RuntimeError) -> &'static str {
    match error {
        RuntimeError::Transport => "transport",
        RuntimeError::Serialization => "serialization",
        RuntimeError::Framing => "framing",
        RuntimeError::Timeout => "timeout",
        RuntimeError::UnknownCall => "unknown_call",
        RuntimeError::Remote { .. } => "remote",
        RuntimeError::Provider => "provider",
        RuntimeError::Handshake => "handshake",
        // `RuntimeError` is `#[non_exhaustive]`: a variant added later gets
        // its name here.
        _ => "other",
    }
}

/// Params already encoded, written as they are: `State::encode_request`
/// takes a value and a format, and this format ignores the value. As the
/// simulator's `Preencoded` does.
struct Preencoded<'a>(&'a [u8]);

impl WireFormat for Preencoded<'_> {
    fn name(&self) -> &'static str {
        "preencoded"
    }

    fn encode<T: serde::Serialize + ?Sized>(
        &self,
        _: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        out.put_slice(self.0);
        Ok(())
    }

    fn decode<'de, T: Deserialize<'de>>(&self, _: &'de [u8]) -> Result<T, RuntimeError> {
        Err(RuntimeError::Serialization)
    }
}

/// A method name as the `&'static str` a [`Call`] holds. Leaks one string
/// per distinct name over the process's life.
fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    match names.get(name) {
        Some(&interned) => interned,
        None => {
            let interned: &'static str = Box::leak(name.into());
            names.insert(interned);
            interned
        }
    }
}
