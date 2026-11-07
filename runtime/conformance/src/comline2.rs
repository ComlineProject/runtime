// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline@2` (runtime#15): its messages (`comline2_message`) and its
//! negotiation steps (`comline2_negotiate`), and the reference's cases for
//! both.

use serde::{Deserialize, Serialize};

use comline_runtime::comline2::*;
use comline_runtime::contract::{HandshakeFrame, Reason, Side, FORMAT_PACKED, FRAMING_DATAGRAM3};

use crate::{Case, Compare, Expect, Hex, Input, Outcome};

/// A negotiation step, as a `comline2_negotiate` case states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// `offer`, the responder reads the initiator's hello and answers.
    AnswerOffer,
    /// `offer`, the initiator reads the answer and gives its verdict.
    ConcludeAnswer,
    /// `symmetric`, either end (`side`) reads the peer's hello and gives
    /// its verdict.
    JudgeHello,
    /// Either end (`side`) reads the peer's verdict, with what it concluded
    /// so far (`pending`).
    Finish,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NegotiateResult {
    Accepted(Negotiated),
    Refused(Refusal),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegotiateIn {
    pub step: Step,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Params>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Hello>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Negotiated>,
    /// The message this end sends, if any (`null`: none). Left out for
    /// `finish`.
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub reply: Option<Option<Message>>,
    pub result: NegotiateResult,
}

/// A field that's there, `null` or not: only leaving it out is `None`.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(d).map(Some)
}

/// Runs `step` on `bytes`, and fills in the reply and result.
pub fn negotiate(mut i: NegotiateIn, bytes: &[u8]) -> Result<NegotiateIn, Outcome> {
    let need = |what: &str| Outcome::Fail(format!("a `{:?}` step needs `{what}`", i.step));
    let side = || i.side.ok_or_else(|| need("side"));
    let local = || -> Result<Comline2, Outcome> {
        Ok(Comline2::new(
            i.params.ok_or_else(|| need("params"))?,
            i.local.clone().ok_or_else(|| need("local"))?,
        ))
    };
    let step = match i.step {
        Step::AnswerOffer => local()?.answer_offer(bytes),
        Step::ConcludeAnswer => local()?.conclude_answer(bytes),
        Step::JudgeHello => local()?.judge_hello(side()?, bytes),
        Step::Finish => {
            let pending = i.pending.clone().ok_or_else(|| need("pending"))?;
            let result = Comline2::finish(side()?, pending, bytes);
            i.result = result_of(result);
            return Ok(i);
        }
    };
    i.reply = Some(step.reply);
    i.result = result_of(step.result);
    Ok(i)
}

fn result_of(result: Result<Negotiated, Refusal>) -> NegotiateResult {
    match result {
        Ok(n) => NegotiateResult::Accepted(n),
        Err(r) => NegotiateResult::Refused(r),
    }
}

/// Whether `cases` show a `comline@2` handshake fact: an accepted
/// negotiation, and refusals for what the fact covers: a line that differs
/// and another package (`schema`: `schema_agreement`); encodings that differ
/// (`encoding_agreement`).
pub(crate) fn agreement_shown(schema: bool, cases: &[&Case]) -> bool {
    let results: Vec<NegotiateResult> = cases
        .iter()
        .filter_map(|c| match &c.input {
            Input::Comline2Negotiate(i) => Some(i.result.clone()),
            _ => None,
        })
        .collect();
    // Refused: the connection, or (under `scope = protocol`) a protocol.
    let refused = |reason: Reason| {
        results.iter().any(|r| match r {
            NegotiateResult::Refused(f) => f.reason == reason,
            NegotiateResult::Accepted(n) => n
                .protocols
                .iter()
                .any(|p| p.state == ProtocolState::Refused { reason }),
        })
    };
    // Accepted: the connection and every protocol on it.
    let accepted = results.iter().any(|r| {
        matches!(r, NegotiateResult::Accepted(n)
            if n.protocols.iter().all(|p| matches!(p.state, ProtocolState::Accepted { .. })))
    });
    accepted
        && if schema {
            refused(Reason::LineMismatch) && refused(Reason::PackageMismatch)
        } else {
            refused(Reason::EncodingMismatch) && refused(Reason::ExchangeMismatch)
        }
}

// ── the reference's cases ──────────────────────────────────────────────────

fn hash(byte: u8) -> Hash128 {
    Hash128([byte; 16])
}

fn protocol(number: u16, line: (u32, u32, u32), shape: u8, functions: &[u16]) -> Protocol {
    Protocol {
        number,
        line: Release::new(line.0, line.1, line.2),
        shape_hash: hash(shape),
        functions: functions.to_vec(),
    }
}

/// The local end of every negotiation case: `pkg` 0.3.0, with protocol 0
/// (line 0.2.0, functions 0–2) and protocol 1 (line 0.1.0, functions 0–1).
fn base(exchange: Exchange) -> Hello {
    Hello {
        exchange,
        package_id: Hash128::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
        version: PackageVersion::release(0, 3, 0),
        encodings: vec![
            Encoding { kind: EncodingKind::Format, param_hash: hash(0x11) },
            Encoding { kind: EncodingKind::Framing, param_hash: hash(0x22) },
            Encoding { kind: EncodingKind::Handshake, param_hash: hash(0x33) },
        ],
        protocols: vec![protocol(0, (0, 2, 0), 0xa0, &[0, 1, 2]), protocol(1, (0, 1, 0), 0xb0, &[0, 1])],
    }
}

fn message_case(id: &str, message: Message) -> Case {
    Case {
        id: format!("handshake-comline2/{id}"),
        bytes: Hex(message.to_bytes()),
        input: Input::Comline2Message(Some(message)),
        expect: Expect::Exact,
        compare: Compare::Bytes,
    }
}

fn reject_case(id: &str, bytes: Vec<u8>) -> Case {
    Case {
        id: format!("handshake-comline2/{id}"),
        input: Input::Comline2Message(None),
        bytes: Hex(bytes),
        expect: Expect::Reject,
        compare: Compare::Bytes,
    }
}

/// A negotiation case, its reply and result the reference's own.
fn negotiate_case(
    id: &str,
    step: Step,
    side: Option<Side>,
    params: Params,
    peer: Vec<u8>,
) -> Case {
    let input = NegotiateIn {
        step,
        side,
        params: Some(params),
        local: Some(Comline2::new(params, base(params.exchange)).hello),
        pending: None,
        reply: None,
        result: NegotiateResult::Refused(Refusal { reason: Reason::Malformed, side: Side::Initiator, protocol: None }),
    };
    let input = negotiate(input, &peer).unwrap_or_else(|o| panic!("reference case `{id}`: {o:?}"));
    Case {
        id: format!("handshake-comline2/{id}"),
        input: Input::Comline2Negotiate(input),
        bytes: Hex(peer),
        expect: Expect::Exact,
        compare: Compare::Bytes,
    }
}

fn finish_case(id: &str, side: Side, pending: Negotiated, verdict: Message) -> Case {
    let input = NegotiateIn {
        step: Step::Finish,
        side: Some(side),
        params: None,
        local: None,
        pending: Some(pending),
        reply: None,
        result: NegotiateResult::Refused(Refusal { reason: Reason::Malformed, side, protocol: None }),
    };
    let bytes = verdict.to_bytes();
    let input = negotiate(input, &bytes).unwrap();
    Case {
        id: format!("handshake-comline2/{id}"),
        input: Input::Comline2Negotiate(input),
        bytes: Hex(bytes),
        expect: Expect::Exact,
        compare: Compare::Bytes,
    }
}

fn hello_bytes(h: Hello) -> Vec<u8> {
    Message::Hello(h.canonical()).to_bytes()
}

/// Every case of the `handshake-comline2` feature.
pub fn cases() -> Vec<Case> {
    let mut cases = Vec::new();

    // ── messages ──
    let hello = base(Exchange::Offer);
    cases.push(message_case("hello", Message::Hello(hello.clone())));
    let mut wide = base(Exchange::Symmetric);
    wide.version = PackageVersion { release: Release::new(1, 200, 70000), pre: "dev.2".into() };
    wide.protocols = vec![
        protocol(0, (1, 0, 0), 0x01, &[]),
        protocol(300, (1, 200, 0), 0x02, &[0, 127, 128, 65535]),
        protocol(65535, (0, 0, 1), 0x03, &[5]),
    ];
    cases.push(message_case("hello-symmetric-wide", Message::Hello(wide)));
    let mut empty = base(Exchange::Offer);
    empty.encodings.clear();
    empty.protocols.clear();
    cases.push(message_case("hello-empty", Message::Hello(empty)));
    cases.push(message_case(
        "answer-accept",
        Message::Answer(Answer::Accept {
            version: PackageVersion::release(0, 2, 1),
            protocols: vec![
                Answered { number: 0, status: Status::Accepted { shape_differs: true, missing: vec![2, 300] } },
                Answered { number: 1, status: Status::Accepted { shape_differs: false, missing: vec![] } },
                Answered { number: 5, status: Status::Refused { reason: Reason::UnknownProtocol } },
            ],
        }),
    ));
    cases.push(message_case(
        "answer-refuse",
        Message::Answer(Answer::Refuse(Refusal {
            reason: Reason::PackageMismatch,
            side: Side::Responder,
            protocol: None,
        })),
    ));
    cases.push(message_case(
        "answer-refuse-protocol",
        Message::Answer(Answer::Refuse(Refusal {
            reason: Reason::LineMismatch,
            side: Side::Responder,
            protocol: Some(300),
        })),
    ));
    cases.push(message_case("verdict-accept", Message::Verdict(Verdict::Accept { refused: vec![] })));
    cases.push(message_case(
        "verdict-accept-refusing",
        Message::Verdict(Verdict::Accept {
            refused: vec![
                RefusedProtocol { number: 0, reason: Reason::SameVersionDiffers },
                RefusedProtocol { number: 1, reason: Reason::NewerConsumer },
            ],
        }),
    ));
    for reason in Reason::ALL {
        let refusal = Refusal {
            reason,
            side: Side::Initiator,
            protocol: reason.per_protocol().then_some(1),
        };
        cases.push(message_case(&format!("verdict-refuse-{}", reason.name()), Message::Verdict(Verdict::Refuse(refusal))));
    }

    // ── malformed messages ──
    let good = Message::Hello(hello.clone()).to_bytes();
    let mut comline1 = Vec::new();
    HandshakeFrame::new(1, FORMAT_PACKED, FRAMING_DATAGRAM3, 0).encode(&mut comline1);
    let with = |at: usize, byte: u8| {
        let mut b = good.clone();
        b[at] = byte;
        b
    };
    let mut trailing = good.clone();
    trailing.push(0);
    let mut unsorted = base(Exchange::Offer);
    unsorted.protocols.swap(0, 1);
    let mut repeated = base(Exchange::Offer);
    repeated.protocols[0].functions = vec![1, 1];
    cases.extend([
        reject_case("reject-empty", Vec::new()),
        reject_case("reject-comline1-frame", comline1.clone()),
        reject_case("reject-bad-magic", with(0, b'X')),
        reject_case("reject-unknown-type", with(3, 9)),
        reject_case("reject-unknown-exchange", with(4, 2)),
        reject_case("reject-truncated", good[..good.len() - 1].to_vec()),
        reject_case("reject-trailing", trailing),
        // Hand-spelled: the encoder sorts, so these are written raw.
        reject_case("reject-unsorted-protocols", raw_hello(&unsorted)),
        reject_case("reject-repeated-function", raw_hello(&repeated)),
        reject_case("reject-unknown-reason", b"CO\x02\x03\x01\x0a\x00".to_vec()),
        reject_case("reject-unknown-side", b"CO\x02\x03\x01\x04\x02".to_vec()),
        reject_case("reject-overlong-number", b"CO\x02\x03\x01\x06\x00\x80\x00".to_vec()),
    ]);

    // ── negotiation: offer, the responder ──
    let offer = Params::default();
    let peer = |f: &dyn Fn(&mut Hello)| {
        let mut h = base(Exchange::Offer);
        f(&mut h);
        hello_bytes(h)
    };
    let answer = |id: &str, params: Params, bytes: Vec<u8>| {
        negotiate_case(id, Step::AnswerOffer, None, params, bytes)
    };
    // A newer consumer: 0.4.0, a function more in protocol 0.
    let newer = |h: &mut Hello| {
        h.version = PackageVersion::release(0, 4, 0);
        h.protocols[0].functions.push(3);
        h.protocols[0].shape_hash = hash(0xa1);
    };
    let package = Params { scope: Scope::Package, ..offer };
    let strict = Params { newer_consumer: NewerConsumer::Refuse, ..offer };
    let warn = Params { same_version_differs: SameVersionDiffers::Warn, ..offer };
    let same_version_differs = |h: &mut Hello| h.protocols[0].shape_hash = hash(0xa9);
    let line_moved = |h: &mut Hello| {
        h.version = PackageVersion::release(0, 4, 0);
        h.protocols[0].line = Release::new(0, 4, 0);
    };
    cases.extend([
        answer("offer-accept", offer, peer(&|_| {})),
        answer("offer-newer-consumer-degrade", offer, peer(&newer)),
        answer("offer-newer-consumer-refuse", strict, peer(&newer)),
        answer("offer-newer-consumer-refuse-package", Params { scope: Scope::Package, ..strict }, peer(&newer)),
        answer("offer-older-consumer", offer, peer(&|h| {
            h.version = PackageVersion::release(0, 2, 5);
            h.protocols[0].functions = vec![0, 1];
            h.protocols[0].shape_hash = hash(0xa2);
        })),
        answer("offer-line-mismatch", offer, peer(&line_moved)),
        answer("offer-line-mismatch-package", package, peer(&line_moved)),
        answer("offer-same-version-differs-refuse", offer, peer(&same_version_differs)),
        answer("offer-same-version-differs-warn", warn, peer(&same_version_differs)),
        answer("offer-unknown-protocol", package, peer(&|h| h.protocols.push(protocol(7, (0, 3, 0), 0xc0, &[0])))),
        answer("offer-only-unknown-protocols", offer, peer(&|h| h.protocols = vec![protocol(7, (0, 3, 0), 0xc0, &[0])])),
        answer("offer-package-mismatch", offer, peer(&|h| h.package_id = hash(0xee))),
        answer("offer-encoding-mismatch", offer, peer(&|h| h.encodings[0].param_hash = hash(0x12))),
        answer("offer-exchange-mismatch", offer, peer(&|h| h.exchange = Exchange::Symmetric)),
        answer("offer-malformed", offer, b"hello".to_vec()),
        answer("offer-handshake-version", offer, comline1),
    ]);

    // ── negotiation: offer, the initiator ──
    let conclude = |id: &str, params: Params, answer: Answer| {
        negotiate_case(id, Step::ConcludeAnswer, None, params, Message::Answer(answer).to_bytes())
    };
    let accepted = |version: PackageVersion, p0: Status| Answer::Accept {
        version,
        protocols: vec![
            Answered { number: 0, status: p0 },
            Answered { number: 1, status: Status::Accepted { shape_differs: false, missing: vec![] } },
        ],
    };
    let older = PackageVersion::release(0, 2, 1);
    let lacks2 = Status::Accepted { shape_differs: true, missing: vec![2] };
    let differs = Status::Accepted { shape_differs: true, missing: vec![] };
    cases.extend([
        conclude("conclude-accept", offer, accepted(PackageVersion::release(0, 3, 0), Status::Accepted { shape_differs: false, missing: vec![] })),
        conclude("conclude-degrade", offer, accepted(older.clone(), lacks2.clone())),
        conclude("conclude-newer-consumer-refuse", strict, accepted(older.clone(), lacks2.clone())),
        conclude("conclude-newer-consumer-refuse-package", Params { scope: Scope::Package, ..strict }, accepted(older, lacks2)),
        conclude("conclude-same-version-differs-refuse", offer, accepted(PackageVersion::release(0, 3, 0), differs.clone())),
        conclude("conclude-same-version-differs-warn", warn, accepted(PackageVersion::release(0, 3, 0), differs)),
        conclude("conclude-protocol-refused", offer, accepted(PackageVersion::release(0, 4, 0), Status::Refused { reason: Reason::LineMismatch })),
        conclude("conclude-refused", offer, Answer::Refuse(Refusal { reason: Reason::EncodingMismatch, side: Side::Responder, protocol: None })),
        negotiate_case("conclude-malformed", Step::ConcludeAnswer, None, offer, b"CO\x02".to_vec()),
        negotiate_case("conclude-exchange-mismatch", Step::ConcludeAnswer, None, offer, hello_bytes(base(Exchange::Symmetric))),
    ]);

    // ── negotiation: symmetric ──
    let symmetric = Params { exchange: Exchange::Symmetric, ..offer };
    let sym_peer = |f: &dyn Fn(&mut Hello)| {
        let mut h = base(Exchange::Symmetric);
        f(&mut h);
        hello_bytes(h)
    };
    let judge = |id: &str, side: Side, params: Params, bytes: Vec<u8>| {
        negotiate_case(id, Step::JudgeHello, Some(side), params, bytes)
    };
    cases.extend([
        judge("symmetric-accept", Side::Initiator, symmetric, sym_peer(&|_| {})),
        judge("symmetric-initiator-degrade", Side::Initiator, symmetric, sym_peer(&|h| {
            h.version = PackageVersion::release(0, 2, 9);
            h.protocols[0].functions = vec![0, 2];
            h.protocols[0].shape_hash = hash(0xa3);
        })),
        judge("symmetric-responder-newer-consumer-refuse", Side::Responder, Params { newer_consumer: NewerConsumer::Refuse, ..symmetric }, sym_peer(&newer)),
        judge("symmetric-line-mismatch", Side::Responder, symmetric, sym_peer(&line_moved)),
        judge("symmetric-unknown-protocol", Side::Initiator, symmetric, sym_peer(&|h| { h.protocols.pop(); })),
        judge("symmetric-same-version-differs-warn", Side::Responder, Params { same_version_differs: SameVersionDiffers::Warn, ..symmetric }, sym_peer(&same_version_differs)),
        judge("symmetric-exchange-mismatch", Side::Responder, symmetric, peer(&|_| {})),
        judge("symmetric-peer-refused", Side::Initiator, symmetric, Message::Answer(Answer::Refuse(Refusal { reason: Reason::ExchangeMismatch, side: Side::Responder, protocol: None })).to_bytes()),
    ]);

    // ── finishing: the peer's verdict ──
    let pending = Negotiated {
        peer_version: PackageVersion::release(0, 3, 0),
        protocols: vec![
            ProtocolOutcome { number: 0, state: ProtocolState::Accepted { peer_lacks: vec![2] } },
            ProtocolOutcome { number: 1, state: ProtocolState::Accepted { peer_lacks: vec![] } },
        ],
        warnings: vec![],
    };
    cases.extend([
        finish_case("finish-accept", Side::Responder, pending.clone(), Message::Verdict(Verdict::Accept { refused: vec![] })),
        finish_case(
            "finish-accept-refusing",
            Side::Responder,
            pending.clone(),
            Message::Verdict(Verdict::Accept { refused: vec![RefusedProtocol { number: 0, reason: Reason::NewerConsumer }] }),
        ),
        finish_case(
            "finish-refused",
            Side::Initiator,
            pending,
            Message::Verdict(Verdict::Refuse(Refusal { reason: Reason::SameVersionDiffers, side: Side::Responder, protocol: Some(0) })),
        ),
    ]);
    cases
}

/// A hello written as given, without sorting: for the cases whose bytes
/// the encoder can't produce.
fn raw_hello(h: &Hello) -> Vec<u8> {
    Message::Hello(h.clone()).to_bytes()
}
