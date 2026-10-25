//! A model test of the connection's sans-IO [`State`]: random sequences of
//! local calls, incoming frames and abandons, run against `State` and against
//! a reference implementation small enough to read at a glance, which must
//! agree after every step.
//!
//! The model is a next request id plus the ids waiting on a reply: a stack
//! for the ordered (nested) `State::default()`, a set for
//! `State::unordered()`. Each step compares the request id a call returns,
//! the frame it writes, the [`Event`] (or error) a received frame gives, and
//! `is_waiting`.
//!
//! The documented rules, pinned beside the model:
//! - in ordered mode, a reply that skips the innermost waiting call is
//!   `Err(Framing)`, and leaves every call waiting;
//! - a reply to an id nobody waits on (never issued, already answered,
//!   abandoned, or one-way) is `Stale`;
//! - a one-way call is never waited on;
//! - a call's request id is spent even when encoding its params fails;
//! - with no room left to wait (`State::with_capacity`), a call that expects
//!   a reply is `Err(Busy)` and spends nothing; a one-way call needs no room;
//! - `encode_request_body` writes the frame `encode_request` writes, and a
//!   frame too big for its fixed buffer is `Err(BufferTooSmall)` and changes
//!   nothing;
//! - a malformed frame is `Err(Framing)` and changes nothing.
//!
//! Request-id wrap at `u64::MAX` isn't covered: `State` starts at id 0 and
//! has no way to start elsewhere, and the test adds no API for it.
//!
//! Both connection framings run: `datagram@2` and `jsonrpc@1`.
//! `PROPTEST_CASES` raises the number of sequences per property.
#![cfg(feature = "std")]

use std::collections::BTreeSet;

use comline_runtime::connection::{Event, State};
use comline_runtime::contract::{
    BufMut, Call, ConnectionFraming, Datagram2Framing, Envelope, Frame, ProtocolNumber, Request,
    RequestCall, RuntimeError, SliceBuf, WireFormat,
};
use comline_runtime::framing::JsonRpcFraming;
use proptest::prelude::*;
use proptest::sample::Index;
use serde::{Deserialize, Serialize};

/// Params already encoded, written as they are: `State::encode_request`
/// takes a `Serialize` value and a format, and this format ignores the
/// value. As the simulator's `Preencoded` does.
struct Preencoded<'a>(&'a [u8]);

impl WireFormat for Preencoded<'_> {
    fn name(&self) -> &'static str {
        "preencoded"
    }
    fn encode<T: Serialize + ?Sized>(
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

/// A format whose every encode fails, for "the id is spent anyway".
struct Failing;

impl WireFormat for Failing {
    fn name(&self) -> &'static str {
        "failing"
    }
    fn encode<T: Serialize + ?Sized>(&self, _: &T, _: &mut dyn BufMut) -> Result<(), RuntimeError> {
        Err(RuntimeError::Serialization)
    }
    fn decode<'de, T: Deserialize<'de>>(&self, _: &'de [u8]) -> Result<T, RuntimeError> {
        Err(RuntimeError::Serialization)
    }
}

/// Method names, `&'static` as [`Call`] wants them.
const METHODS: [&str; 4] = ["ping", "send", "chat.Chat/send", "a"];

/// JSON texts, for params and payloads under `jsonrpc@1`, whose frames carry
/// them inline. Each is written as `serde_json` reads it back: no spaces.
const JSON_TEXTS: [&str; 6] = ["null", "1", "\"x\"", "[1,2]", "{\"a\":true}", "-3.5"];

/// A body: raw bytes under `datagram@2`, a JSON text under `jsonrpc@1`.
#[derive(Debug, Clone)]
struct Body {
    raw: Vec<u8>,
    json: Index,
}

impl Body {
    fn bytes(&self, json: bool) -> Vec<u8> {
        if json {
            self.json.get(&JSON_TEXTS).as_bytes().to_vec()
        } else {
            self.raw.clone()
        }
    }
}

#[derive(Debug, Clone)]
enum EnvelopeKind {
    Ok(Body),
    Err(u16, Body),
    Fault,
}

/// Which id an incoming reply answers.
#[derive(Debug, Clone)]
enum ReplyTo {
    /// A waiting call: in ordered mode index 0 is the innermost, any other a
    /// deeper one. Falls back to `Spent` when nothing waits.
    Waiting(Index),
    /// An id this side handed out and no longer waits on: answered,
    /// abandoned, one-way, or whose encoding failed. Falls back to `Never`.
    Spent(Index),
    /// An id this side hasn't handed out yet: the next id plus this.
    Never(u64),
}

/// How a local call is made.
#[derive(Debug, Clone)]
enum How {
    /// `encode_request`, with a format that writes the params.
    Serialized,
    /// `encode_request`, with a format whose encode fails.
    Failing,
    /// `encode_request_body`, into a fixed buffer with room for the frame.
    Body,
    /// `encode_request_body`, into a fixed buffer at least a byte short.
    Short(Index),
}

#[derive(Debug, Clone)]
enum Step {
    /// A local call, made `how`.
    Call {
        protocol: ProtocolNumber,
        call_id: u16,
        method: Index,
        one_way: bool,
        params: Body,
        how: How,
    },
    /// The peer calls this side.
    Request {
        protocol: ProtocolNumber,
        call_id: u16,
        method: Index,
        request_id: u64,
        one_way: bool,
        params: Body,
    },
    Reply {
        to: ReplyTo,
        protocol: ProtocolNumber,
        envelope: EnvelopeKind,
    },
    /// Give up on a waiting call, or (`ReplyTo::Spent`/`Never`) on an id
    /// nobody waits on, which changes nothing.
    Abandon(ReplyTo),
    Malformed(Malformed),
}

/// A frame no framing reads, chosen per framing.
#[derive(Debug, Clone)]
enum Malformed {
    Empty,
    /// `datagram@2`: a kind byte other than request (0) or reply (1).
    /// `jsonrpc@1`: text that isn't JSON.
    BadStart(u8, Vec<u8>),
    /// A valid reply frame cut short by at least one byte.
    Truncated(Index),
    /// `datagram@2`: a reply whose envelope tag is unknown. `jsonrpc@1`: a
    /// reply with no `id`.
    BadReply(u64),
}

fn body() -> impl Strategy<Value = Body> {
    (prop::collection::vec(any::<u8>(), 0..12), any::<Index>())
        .prop_map(|(raw, json)| Body { raw, json })
}

fn envelope() -> impl Strategy<Value = EnvelopeKind> {
    prop_oneof![
        body().prop_map(EnvelopeKind::Ok),
        (any::<u16>(), body()).prop_map(|(id, b)| EnvelopeKind::Err(id, b)),
        Just(EnvelopeKind::Fault),
    ]
}

fn reply_to() -> impl Strategy<Value = ReplyTo> {
    prop_oneof![
        4 => any::<Index>().prop_map(ReplyTo::Waiting),
        2 => any::<Index>().prop_map(ReplyTo::Spent),
        1 => prop_oneof![0..1000u64, Just(u64::MAX / 2)].prop_map(ReplyTo::Never),
    ]
}

fn how() -> impl Strategy<Value = How> {
    prop_oneof![
        8 => Just(How::Serialized),
        1 => Just(How::Failing),
        8 => Just(How::Body),
        2 => any::<Index>().prop_map(How::Short),
    ]
}

/// Room for waiting calls: as many as needed, or a few (none at all
/// included), so `Busy` comes up.
fn capacity() -> impl Strategy<Value = Option<usize>> {
    prop_oneof![2 => Just(None), 1 => (0..4usize).prop_map(Some)]
}

fn protocol() -> impl Strategy<Value = ProtocolNumber> {
    prop_oneof![0..4u16, any::<u16>()]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (protocol(), any::<u16>(), any::<Index>(), prop::bool::weighted(0.25), body(), how())
            .prop_map(|(protocol, call_id, method, one_way, params, how)| Step::Call {
                protocol, call_id, method, one_way, params, how,
            }),
        2 => (protocol(), any::<u16>(), any::<Index>(), any::<u64>(), any::<bool>(), body())
            .prop_map(|(protocol, call_id, method, request_id, one_way, params)| Step::Request {
                protocol, call_id, method, request_id, one_way, params,
            }),
        6 => (reply_to(), protocol(), envelope())
            .prop_map(|(to, protocol, envelope)| Step::Reply { to, protocol, envelope }),
        2 => reply_to().prop_map(Step::Abandon),
        1 => prop_oneof![
            Just(Malformed::Empty),
            (any::<u8>(), prop::collection::vec(any::<u8>(), 0..12)).prop_map(|(b, r)| Malformed::BadStart(b, r)),
            any::<Index>().prop_map(Malformed::Truncated),
            any::<u64>().prop_map(Malformed::BadReply),
        ]
        .prop_map(Step::Malformed),
    ]
}

/// The reference implementation.
#[derive(Debug)]
struct Model {
    ordered: bool,
    /// How many calls may wait at once.
    capacity: usize,
    next_id: u64,
    /// Waiting ids, innermost last (ordered) or in issue order (unordered,
    /// where the order means nothing).
    waiting: Vec<u64>,
    /// Ids handed out and no longer (or never) waited on.
    spent: BTreeSet<u64>,
}

/// What receiving a reply to `id` must give.
#[derive(Debug, PartialEq)]
enum Expected {
    Reply,
    Stale,
    Framing,
}

impl Model {
    /// What a call made `how` gives: `Busy` before anything else, then a
    /// short buffer (nothing spent), then a failed encode (its id spent).
    fn call(&mut self, expects_reply: bool, how: &How) -> Result<u64, RuntimeError> {
        if expects_reply && self.waiting.len() >= self.capacity {
            return Err(RuntimeError::Busy);
        }
        if let How::Short(_) = how {
            return Err(RuntimeError::BufferTooSmall);
        }
        let id = self.next_id;
        self.next_id += 1;
        let failing = matches!(how, How::Failing);
        if expects_reply && !failing {
            self.waiting.push(id);
        } else {
            self.spent.insert(id);
        }
        if failing {
            Err(RuntimeError::Serialization)
        } else {
            Ok(id)
        }
    }

    fn receive_reply(&mut self, id: u64) -> Expected {
        let at = self.waiting.iter().position(|&w| w == id);
        match at {
            None => Expected::Stale,
            Some(at) if self.ordered && at + 1 != self.waiting.len() => Expected::Framing,
            Some(at) => {
                self.waiting.remove(at);
                self.spent.insert(id);
                Expected::Reply
            }
        }
    }

    fn abandon(&mut self, id: u64) {
        if let Some(at) = self.waiting.iter().position(|&w| w == id) {
            self.waiting.remove(at);
            self.spent.insert(id);
        }
    }

    fn is_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// The id `to` names, now. The second value: whether it names a waiting
    /// call deeper than the innermost (ordered mode only).
    fn resolve(&self, to: &ReplyTo) -> (u64, bool) {
        match to {
            ReplyTo::Waiting(index) if !self.waiting.is_empty() => {
                // Index 0 is the innermost: the last pushed.
                let from_top = index.index(self.waiting.len());
                let at = self.waiting.len() - 1 - from_top;
                (self.waiting[at], self.ordered && from_top != 0)
            }
            ReplyTo::Waiting(index) | ReplyTo::Spent(index) if !self.spent.is_empty() => {
                let at = index.index(self.spent.len());
                (*self.spent.iter().nth(at).unwrap(), false)
            }
            ReplyTo::Never(offset) => (self.next_id + offset, false),
            _ => (self.next_id, false),
        }
    }
}

fn envelope_of<'a>(kind: &EnvelopeKind, bytes: &'a [u8]) -> Envelope<'a> {
    match kind {
        EnvelopeKind::Ok(_) => Envelope::Ok(bytes),
        EnvelopeKind::Err(id, _) => Envelope::Err {
            id: *id,
            body: bytes,
        },
        EnvelopeKind::Fault => Envelope::Fault,
    }
}

fn run<F: ConnectionFraming>(
    ordered: bool,
    capacity: Option<usize>,
    steps: &[Step],
) -> Result<(), TestCaseError> {
    let framing = F::default();
    let json = framing.name() == comline_runtime::contract::FRAMING_JSONRPC;
    let mut state = match (ordered, capacity) {
        (true, None) => State::default(),
        (false, None) => State::unordered(),
        (true, Some(n)) => State::with_capacity(n),
        (false, Some(n)) => State::unordered_with_capacity(n),
    };
    let mut model = Model {
        ordered,
        capacity: capacity.unwrap_or(usize::MAX),
        next_id: 0,
        waiting: Vec::new(),
        spent: BTreeSet::new(),
    };
    let mut out = Vec::new();
    // Valid reply frames sent so far, for `Malformed::Truncated`.
    let mut replies: Vec<Vec<u8>> = Vec::new();

    for (n, step) in steps.iter().enumerate() {
        match step {
            Step::Call {
                protocol,
                call_id,
                method,
                one_way,
                params,
                how,
            } => {
                let params = params.bytes(json);
                let call = Call::new(*call_id, method.get(&METHODS));
                let got = match how {
                    How::Serialized => state.encode_request(
                        &framing,
                        *protocol,
                        call,
                        &(),
                        &Preencoded(&params),
                        !one_way,
                        &mut out,
                    ),
                    How::Failing => state.encode_request(
                        &framing,
                        *protocol,
                        call,
                        &(),
                        &Failing,
                        !one_way,
                        &mut out,
                    ),
                    How::Body | How::Short(_) => {
                        // The frame's size, so the fixed buffer can be cut
                        // short of it: the id it would get is the model's next.
                        let mut whole = Vec::new();
                        framing.encode_request_body(
                            *protocol,
                            *call_id,
                            call.name,
                            model.next_id,
                            *one_way,
                            &params,
                            &mut whole,
                        );
                        let room = match how {
                            How::Short(cut) => cut.index(whole.len()),
                            _ => whole.len(),
                        };
                        let mut buf = vec![0u8; room];
                        let mut fixed = SliceBuf::new(&mut buf);
                        let got = state.encode_request_body(
                            &framing, *protocol, *call_id, call.name, &params, !one_way, &mut fixed,
                        );
                        if let Ok(id) = got {
                            // Byte for byte what `encode_request` writes.
                            let mut serialized = Vec::new();
                            framing
                                .encode_request(
                                    *protocol,
                                    call,
                                    id,
                                    *one_way,
                                    &(),
                                    &Preencoded(&params),
                                    &mut serialized,
                                )
                                .unwrap();
                            prop_assert_eq!(fixed.written(), &serialized[..], "step {}", n);
                        }
                        out.clear();
                        out.extend_from_slice(fixed.written());
                        got
                    }
                };
                let expected = model.call(!one_way, how);
                prop_assert_eq!(got, expected, "step {}: {:?}", n, how);
                if let Ok(id) = expected {
                    // The frame is the framing's request, with that id.
                    let (expect_protocol, expect_call, expect_id, expect_one_way) = if json {
                        (
                            None,
                            RequestCall::Name(call.name.into()),
                            if *one_way { 0 } else { id },
                            *one_way,
                        )
                    } else {
                        (Some(*protocol), RequestCall::Id(*call_id), id, false)
                    };
                    prop_assert_eq!(
                        framing.decode(&out),
                        Some(Frame::Request {
                            protocol: expect_protocol,
                            request: Request {
                                call: expect_call,
                                request_id: expect_id,
                                params: &params
                            },
                            one_way: expect_one_way,
                        }),
                        "step {}: the frame",
                        n
                    );
                }
            }
            Step::Request {
                protocol,
                call_id,
                method,
                request_id,
                one_way,
                params,
            } => {
                let params = params.bytes(json);
                let call = Call::new(*call_id, method.get(&METHODS));
                let mut frame = Vec::new();
                framing
                    .encode_request(
                        *protocol,
                        call,
                        *request_id,
                        *one_way,
                        &(),
                        &Preencoded(&params),
                        &mut frame,
                    )
                    .unwrap();
                let expected = if json {
                    Event::Request {
                        protocol: None,
                        request: Request {
                            call: RequestCall::Name(call.name.into()),
                            request_id: if *one_way { 0 } else { *request_id },
                            params: &params,
                        },
                        one_way: *one_way,
                    }
                } else {
                    Event::Request {
                        protocol: Some(*protocol),
                        request: Request {
                            call: RequestCall::Id(*call_id),
                            request_id: *request_id,
                            params: &params,
                        },
                        one_way: false,
                    }
                };
                prop_assert_eq!(state.receive(&framing, &frame), Ok(expected), "step {}", n);
            }
            Step::Reply {
                to,
                protocol,
                envelope,
            } => {
                let (id, deeper) = model.resolve(to);
                let body = match envelope {
                    EnvelopeKind::Ok(b) | EnvelopeKind::Err(_, b) => b.bytes(json),
                    EnvelopeKind::Fault => Vec::new(),
                };
                let sent = envelope_of(envelope, &body);
                let mut frame = Vec::new();
                framing.encode_reply(*protocol, id, &sent, &mut frame);
                let got = state.receive(&framing, &frame);
                let expected = model.receive_reply(id);
                if deeper {
                    // The rule itself, not only the model's word for it.
                    prop_assert_eq!(&expected, &Expected::Framing);
                }
                match expected {
                    Expected::Reply => {
                        prop_assert_eq!(
                            got,
                            Ok(Event::Reply {
                                request_id: id,
                                envelope: sent
                            }),
                            "step {}",
                            n
                        )
                    }
                    Expected::Stale => {
                        prop_assert_eq!(got, Ok(Event::Stale), "step {}: reply to {}", n, id)
                    }
                    Expected::Framing => {
                        prop_assert_eq!(got, Err(RuntimeError::Framing), "step {}", n)
                    }
                }
                replies.push(frame);
            }
            Step::Abandon(to) => {
                let (id, _) = model.resolve(to);
                state.abandon(id);
                model.abandon(id);
            }
            Step::Malformed(what) => {
                let frame = malformed(json, what, &replies);
                prop_assert_eq!(
                    framing.decode(&frame),
                    None,
                    "step {}: {:?} decodes",
                    n,
                    frame
                );
                prop_assert_eq!(
                    state.receive(&framing, &frame),
                    Err(RuntimeError::Framing),
                    "step {}",
                    n
                );
            }
        }
        prop_assert_eq!(
            state.is_waiting(),
            model.is_waiting(),
            "step {}: is_waiting",
            n
        );
    }

    // Everything still waiting is answerable, innermost first, and then
    // nothing is.
    while let Some(&id) = model.waiting.last() {
        let mut frame = Vec::new();
        framing.encode_reply(0, id, &Envelope::Fault, &mut frame);
        prop_assert_eq!(
            state.receive(&framing, &frame),
            Ok(Event::Reply {
                request_id: id,
                envelope: Envelope::Fault
            }),
            "draining {}",
            id
        );
        model.receive_reply(id);
    }
    prop_assert!(!state.is_waiting());
    Ok(())
}

fn malformed(json: bool, what: &Malformed, replies: &[Vec<u8>]) -> Vec<u8> {
    match what {
        Malformed::Empty => Vec::new(),
        Malformed::BadStart(byte, rest) if json => {
            // `!` starts no JSON value.
            let mut frame = vec![b'!', *byte];
            frame.extend_from_slice(rest);
            frame
        }
        Malformed::BadStart(byte, rest) => {
            let mut frame = vec![2u8.saturating_add(*byte % 254)];
            frame.extend_from_slice(rest);
            frame
        }
        Malformed::Truncated(index) => match replies {
            [] => Vec::new(),
            _ => {
                // Under `datagram@2` a reply's envelope ends wherever the
                // frame does, so only a cut inside the header (at least 10
                // bytes: kind, protocol, request id) is sure to be malformed.
                // Any cut of a JSON object is.
                let full = index.get(replies);
                let keep = index.index(if json { full.len() } else { full.len().min(10) });
                full[..keep].to_vec()
            }
        },
        Malformed::BadReply(id) if json => {
            format!(r#"{{"jsonrpc":"2.0","result":{id}}}"#).into_bytes()
        }
        Malformed::BadReply(id) => {
            let mut frame = vec![1, 0];
            frame.extend_from_slice(&id.to_le_bytes());
            frame.push(0xee);
            frame
        }
    }
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(step(), 0..64)
}

proptest! {
    #[test]
    fn ordered_datagram2_agrees_with_the_model(capacity in capacity(), steps in steps()) {
        run::<Datagram2Framing>(true, capacity, &steps)?;
    }

    #[test]
    fn unordered_datagram2_agrees_with_the_model(capacity in capacity(), steps in steps()) {
        run::<Datagram2Framing>(false, capacity, &steps)?;
    }

    #[test]
    fn ordered_jsonrpc_agrees_with_the_model(capacity in capacity(), steps in steps()) {
        run::<JsonRpcFraming>(true, capacity, &steps)?;
    }

    #[test]
    fn unordered_jsonrpc_agrees_with_the_model(capacity in capacity(), steps in steps()) {
        run::<JsonRpcFraming>(false, capacity, &steps)?;
    }
}

/// The rules, each on its own fixed sequence, so a regression names the rule.
mod rules {
    use super::*;

    fn call(state: &mut State, expects_reply: bool) -> u64 {
        let mut out = Vec::new();
        state
            .encode_request(
                &Datagram2Framing,
                0,
                Call::from(0),
                &(),
                &Preencoded(b""),
                expects_reply,
                &mut out,
            )
            .unwrap()
    }

    fn reply(id: u64) -> Vec<u8> {
        let mut frame = Vec::new();
        Datagram2Framing.encode_reply(0, id, &Envelope::Ok(b""), &mut frame);
        frame
    }

    #[test]
    fn a_one_way_call_is_never_waited_on() {
        for mut state in [State::default(), State::unordered()] {
            let id = call(&mut state, false);
            assert!(!state.is_waiting());
            assert_eq!(
                state.receive(&Datagram2Framing, &reply(id)),
                Ok(Event::Stale)
            );
        }
    }

    #[test]
    fn a_failed_encode_spends_its_id() {
        for mut state in [State::default(), State::unordered()] {
            let mut out = Vec::new();
            let failed = state.encode_request(
                &Datagram2Framing,
                0,
                Call::from(0),
                &(),
                &Failing,
                true,
                &mut out,
            );
            assert_eq!(failed, Err(RuntimeError::Serialization));
            assert!(!state.is_waiting());
            assert_eq!(call(&mut state, true), 1);
            assert_eq!(
                state.receive(&Datagram2Framing, &reply(0)),
                Ok(Event::Stale)
            );
        }
    }

    #[test]
    fn a_full_state_is_busy_and_spends_nothing() {
        for mut state in [State::with_capacity(1), State::unordered_with_capacity(1)] {
            assert_eq!(call(&mut state, true), 0);
            let mut out = Vec::new();
            let busy = state.encode_request(
                &Datagram2Framing,
                0,
                Call::from(0),
                &(),
                &Preencoded(b""),
                true,
                &mut out,
            );
            assert_eq!(busy, Err(RuntimeError::Busy));
            assert!(out.is_empty());
            // A one-way call needs no room.
            assert_eq!(call(&mut state, false), 1);
            assert!(matches!(
                state.receive(&Datagram2Framing, &reply(0)),
                Ok(Event::Reply { request_id: 0, .. })
            ));
            assert_eq!(call(&mut state, true), 2);
        }
    }

    #[test]
    fn a_frame_too_big_for_its_buffer_commits_nothing() {
        for mut state in [State::default(), State::unordered()] {
            let mut room = [0u8; 12];
            let mut fixed = SliceBuf::new(&mut room);
            let short =
                state.encode_request_body(&Datagram2Framing, 0, 0, "", b"params", true, &mut fixed);
            assert_eq!(short, Err(RuntimeError::BufferTooSmall));
            assert!(!state.is_waiting());
            assert_eq!(call(&mut state, true), 0);
        }
    }

    #[test]
    fn a_skipping_reply_is_a_framing_error_and_keeps_both_waiting() {
        let mut state = State::default();
        let outer = call(&mut state, true);
        let inner = call(&mut state, true);
        assert_eq!(
            state.receive(&Datagram2Framing, &reply(outer)),
            Err(RuntimeError::Framing)
        );
        assert!(
            matches!(state.receive(&Datagram2Framing, &reply(inner)), Ok(Event::Reply { request_id, .. }) if request_id == inner)
        );
        assert!(
            matches!(state.receive(&Datagram2Framing, &reply(outer)), Ok(Event::Reply { request_id, .. }) if request_id == outer)
        );
    }

    #[test]
    fn abandoning_the_innermost_lets_the_next_one_answer() {
        let mut state = State::default();
        let outer = call(&mut state, true);
        let inner = call(&mut state, true);
        state.abandon(inner);
        assert_eq!(
            state.receive(&Datagram2Framing, &reply(inner)),
            Ok(Event::Stale)
        );
        assert!(matches!(
            state.receive(&Datagram2Framing, &reply(outer)),
            Ok(Event::Reply { .. })
        ));
        assert!(!state.is_waiting());
    }
}
