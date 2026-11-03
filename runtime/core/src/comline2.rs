//! `comline@2`: the compatibility-checking handshake (runtime#15).
//!
//! Where `comline@1` refuses any peer whose IR differs, `comline@2` compares
//! what core's history records about each version (`comline_core::compat`):
//! the package id, each protocol's wire number, **line** (the release it
//! last broke at) and wire-shape hash, each function's wire number, and each
//! encoding's parameter hash. Lines decide compatibility; the shape hash
//! catches two ends that claim the same version but differ. The namespace
//! is never compared.
//!
//! Four parameters, defaults first. `exchange` is a wire parameter, which
//! both ends must share (it is part of the handshake's parameter hash and
//! of every message); the others are each end's own policy.
//!
//! - `scope = protocol | package`: a protocol-scope refusal refuses that
//!   protocol alone, or the whole connection.
//! - `newer_consumer = degrade | refuse`: when the consumer has functions
//!   the provider lacks, connect anyway (calls to them fail locally with
//!   [`RuntimeError::Unavailable`], never reaching the wire), or refuse.
//! - `same_version_differs = refuse | warn`: when both claim the same
//!   package version but a protocol's shape hash differs.
//! - `exchange = offer | symmetric`: the initiator offers and the responder
//!   answers, or both declare and both judge.
//!
//! The messages and their layout are in [`Message`]; the sans-IO steps are
//! on [`Comline2`], and [`Comline2Handshake`] runs them over a
//! [`Transport`] (and, with `async`, an `AsyncTransport`).

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::contract::{
    put_varint, take_varint, BufMut, Reason, RuntimeError, Side, HANDSHAKE_COMLINE2,
};
use crate::handshake::{Facts, Handshake};
use crate::transport::Transport;
#[cfg(feature = "async")]
use crate::async_connection::AsyncTransport;
#[cfg(feature = "async")]
use crate::handshake::AsyncHandshake;
#[cfg(feature = "async")]
use futures_util::{SinkExt, StreamExt};

const MAGIC: [u8; 2] = *b"CO";
/// The handshake version byte. `comline@1`'s frames carry 1.
const VERSION: u8 = 2;

const HELLO: u8 = 1;
const ANSWER: u8 = 2;
const VERDICT: u8 = 3;

const ACCEPT: u8 = 0;
const REFUSE: u8 = 1;

const ACCEPTED: u8 = 0;
const ACCEPTED_SHAPE_DIFFERS: u8 = 1;
const REFUSED: u8 = 2;

// ── parameters ─────────────────────────────────────────────────────────────

/// `scope`: what a protocol-scope refusal refuses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// That protocol alone; the connection carries the others.
    #[default]
    Protocol,
    /// The whole connection.
    Package,
}

/// `newer_consumer`: what to do when the consumer has functions the
/// provider lacks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewerConsumer {
    /// Connect; calls to the missing functions fail locally.
    #[default]
    Degrade,
    /// Refuse the protocol ([`Reason::NewerConsumer`]).
    Refuse,
}

/// `same_version_differs`: what to do when both ends claim the same package
/// version but a protocol's wire shape differs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameVersionDiffers {
    /// Refuse the protocol ([`Reason::SameVersionDiffers`]).
    #[default]
    Refuse,
    /// Connect, and report it in [`Negotiated::warnings`].
    Warn,
}

/// `exchange`: who sends what. A wire parameter: both ends must share it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exchange {
    /// The initiator sends a [`Hello`]; the responder judges it and sends an
    /// [`Answer`]; the initiator judges that and sends a [`Verdict`].
    #[default]
    Offer,
    /// Both send a [`Hello`], both judge the other's, both send a
    /// [`Verdict`].
    Symmetric,
}

impl Exchange {
    fn code(self) -> u8 {
        match self {
            Exchange::Offer => 0,
            Exchange::Symmetric => 1,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Exchange::Offer),
            1 => Some(Exchange::Symmetric),
            _ => None,
        }
    }
}

/// The four parameters, as `config.idp` declares them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub scope: Scope,
    pub newer_consumer: NewerConsumer,
    pub same_version_differs: SameVersionDiffers,
    pub exchange: Exchange,
}

// ── facts ──────────────────────────────────────────────────────────────────

/// A 128-bit hash: the first 16 bytes of core's blake3 shape and parameter
/// hashes. Hex in JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash128(pub [u8; 16]);

impl Hash128 {
    /// From hex, as core's facts write a hash: the first 32 digits, so a
    /// full 64-digit blake3 hash is cut to 128 bits.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.as_bytes().get(..32)?;
        let mut out = [0u8; 16];
        for (i, pair) in hex.chunks(2).enumerate() {
            let digit = |b: u8| (b as char).to_digit(16);
            out[i] = (digit(pair[0])? * 16 + digit(pair[1])?) as u8;
        }
        Some(Hash128(out))
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(32);
        for b in self.0 {
            s.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
            s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap());
        }
        s
    }
}

impl Serialize for Hash128 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash128 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        if hex.len() != 32 {
            return Err(serde::de::Error::custom("a 128-bit hash is 32 hex digits"));
        }
        Hash128::from_hex(&hex).ok_or_else(|| serde::de::Error::custom("not hex"))
    }
}

/// A release, `major.minor.patch`: a protocol's line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Release {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Release {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch }
    }
}

/// A package version: a release, and a pre-release (`dev.2`), empty for a
/// release.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVersion {
    #[serde(flatten)]
    pub release: Release,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pre: String,
}

impl PackageVersion {
    pub fn release(major: u32, minor: u32, patch: u32) -> Self {
        Self { release: Release::new(major, minor, patch), pre: String::new() }
    }
}

/// What an encoding is: a format, a framing, or the handshake itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodingKind {
    Format = 0,
    Framing = 1,
    Handshake = 2,
}

impl EncodingKind {
    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(EncodingKind::Format),
            1 => Some(EncodingKind::Framing),
            2 => Some(EncodingKind::Handshake),
            _ => None,
        }
    }
}

/// An encoding in play: its kind and core's parameter hash, which covers
/// its name and parameters (`EncodingFacts.param_hash`). The handshake's
/// own entry covers `exchange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Encoding {
    pub kind: EncodingKind,
    pub param_hash: Hash128,
}

/// One protocol's facts: its wire number, line and shape hash, and its
/// functions' wire numbers, ascending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocol {
    pub number: u16,
    pub line: Release,
    pub shape_hash: Hash128,
    pub functions: Vec<u16>,
}

/// What an end declares: its parameters' wire part, the package id and
/// version, the encodings, and the protocols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub exchange: Exchange,
    /// `id` in `config.idp`: 32 hex digits, 16 bytes on the wire.
    pub package_id: Hash128,
    pub version: PackageVersion,
    /// Ascending by kind, then hash.
    pub encodings: Vec<Encoding>,
    /// Ascending by number.
    pub protocols: Vec<Protocol>,
}

impl Hello {
    /// Sorts encodings, protocols and functions into the order the wire
    /// requires.
    pub fn canonical(mut self) -> Self {
        self.encodings.sort();
        self.encodings.dedup();
        self.protocols.sort_by_key(|p| p.number);
        for p in &mut self.protocols {
            p.functions.sort_unstable();
            p.functions.dedup();
        }
        self
    }

    fn protocol(&self, number: u16) -> Option<&Protocol> {
        self.protocols
            .binary_search_by_key(&number, |p| p.number)
            .ok()
            .map(|i| &self.protocols[i])
    }
}

// ── messages ───────────────────────────────────────────────────────────────

/// A refusal: why, which end refused, and the protocol when the reason is
/// about one ([`Reason::per_protocol`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub reason: Reason,
    pub side: Side,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u16>,
}

impl Refusal {
    pub fn error(&self) -> RuntimeError {
        RuntimeError::Refused { reason: self.reason, side: self.side }
    }
}

/// The responder's judgement of one offered protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Accepted. `shape_differs`: the shape hashes differ (which matters to
    /// the initiator's `same_version_differs` when the versions are equal);
    /// `missing`: the offered functions the responder lacks.
    Accepted { shape_differs: bool, missing: Vec<u16> },
    Refused { reason: Reason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answered {
    pub number: u16,
    pub status: Status,
}

/// The responder's reply to an offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// The responder's package version, and every offered protocol, in the
    /// offer's order.
    Accept { version: PackageVersion, protocols: Vec<Answered> },
    Refuse(Refusal),
}

/// A protocol an end refuses, in an accepting [`Verdict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedProtocol {
    pub number: u16,
    pub reason: Reason,
}

/// An end's last word: accept (refusing some protocols, ascending), or
/// refuse the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accept { refused: Vec<RefusedProtocol> },
    Refuse(Refusal),
}

/// A `comline@2` message. Every one starts `"CO"`, the version byte `2`
/// and a type byte; numbers are unsigned LEB128 varints in their shortest
/// form; a hash is 16 bytes; a release is three varints. A message ends
/// where its fields do: trailing bytes are malformed.
///
/// - **Hello** (type 1): `[exchange u8][package id 16][version: major,
///   minor, patch varints, pre-release length varint + UTF-8][encodings:
///   count, then each kind u8 + param hash 16][protocols: count, then each
///   number, line (3 varints), shape hash 16, functions: count + numbers]`.
///   Encodings ascend by (kind, hash); protocols and functions by number.
/// - **Answer** (type 2): `[0][version][count, then each number + status
///   u8: 0 accepted, 1 accepted with the shape differing, each followed by
///   the missing functions (count + numbers); 2 refused + reason u8]`, or
///   `[1][refusal]`.
/// - **Verdict** (type 3): `[0][count, then each number + reason u8]`, or
///   `[1][refusal]`.
/// - **Refusal:** `[reason u8][side u8]`, then the protocol number when the
///   reason is about one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "message", rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Answer(Answer),
    Verdict(Verdict),
}

fn put_u32(mut value: u32, out: &mut dyn BufMut) {
    while value >= 0x80 {
        out.put_u8((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.put_u8(value as u8);
}

fn put_release(r: &Release, out: &mut dyn BufMut) {
    put_u32(r.major, out);
    put_u32(r.minor, out);
    put_u32(r.patch, out);
}

fn put_version(v: &PackageVersion, out: &mut dyn BufMut) {
    put_release(&v.release, out);
    put_u32(v.pre.len() as u32, out);
    out.put_slice(v.pre.as_bytes());
}

fn put_numbers(numbers: &[u16], out: &mut dyn BufMut) {
    put_u32(numbers.len() as u32, out);
    for n in numbers {
        put_varint(*n, out);
    }
}

fn put_refusal(r: &Refusal, out: &mut dyn BufMut) {
    out.put_u8(r.reason.code());
    out.put_u8(r.side.code());
    if r.reason.per_protocol() {
        put_varint(r.protocol.unwrap_or(0), out);
    }
}

impl Message {
    pub fn encode(&self, out: &mut dyn BufMut) {
        out.put_slice(&MAGIC);
        out.put_u8(VERSION);
        match self {
            Message::Hello(h) => {
                out.put_u8(HELLO);
                out.put_u8(h.exchange.code());
                out.put_slice(&h.package_id.0);
                put_version(&h.version, out);
                put_u32(h.encodings.len() as u32, out);
                for e in &h.encodings {
                    out.put_u8(e.kind as u8);
                    out.put_slice(&e.param_hash.0);
                }
                put_u32(h.protocols.len() as u32, out);
                for p in &h.protocols {
                    put_varint(p.number, out);
                    put_release(&p.line, out);
                    out.put_slice(&p.shape_hash.0);
                    put_numbers(&p.functions, out);
                }
            }
            Message::Answer(a) => {
                out.put_u8(ANSWER);
                match a {
                    Answer::Accept { version, protocols } => {
                        out.put_u8(ACCEPT);
                        put_version(version, out);
                        put_u32(protocols.len() as u32, out);
                        for p in protocols {
                            put_varint(p.number, out);
                            match &p.status {
                                Status::Accepted { shape_differs, missing } => {
                                    out.put_u8(if *shape_differs {
                                        ACCEPTED_SHAPE_DIFFERS
                                    } else {
                                        ACCEPTED
                                    });
                                    put_numbers(missing, out);
                                }
                                Status::Refused { reason } => {
                                    out.put_u8(REFUSED);
                                    out.put_u8(reason.code());
                                }
                            }
                        }
                    }
                    Answer::Refuse(r) => {
                        out.put_u8(REFUSE);
                        put_refusal(r, out);
                    }
                }
            }
            Message::Verdict(v) => {
                out.put_u8(VERDICT);
                match v {
                    Verdict::Accept { refused } => {
                        out.put_u8(ACCEPT);
                        put_u32(refused.len() as u32, out);
                        for r in refused {
                            put_varint(r.number, out);
                            out.put_u8(r.reason.code());
                        }
                    }
                    Verdict::Refuse(r) => {
                        out.put_u8(REFUSE);
                        put_refusal(r, out);
                    }
                }
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }

    /// Parse one. `Err(Reason::HandshakeVersion)` for a frame of another
    /// version of the handshake (`"CO"` and another version byte, such as a
    /// `comline@1` frame); `Err(Reason::Malformed)` for anything else that
    /// isn't a well-formed message in canonical form.
    pub fn decode(bytes: &[u8]) -> Result<Message, Reason> {
        match bytes {
            [b'C', b'O', VERSION, ..] => {}
            [b'C', b'O', _, ..] => return Err(Reason::HandshakeVersion),
            _ => return Err(Reason::Malformed),
        }
        let mut r = Reader(&bytes[3..]);
        let message = r.message().ok_or(Reason::Malformed)?;
        if r.0.is_empty() {
            Ok(message)
        } else {
            Err(Reason::Malformed)
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn hash(&mut self) -> Option<Hash128> {
        Some(Hash128(self.take(16)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        let mut value: u64 = 0;
        for i in 0..5 {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                if i > 0 && byte == 0 {
                    return None;
                }
                return u32::try_from(value).ok();
            }
        }
        None
    }

    fn number(&mut self) -> Option<u16> {
        let (n, rest) = take_varint(self.0)?;
        self.0 = rest;
        Some(n)
    }

    /// A count, bounded by the bytes left (each item takes at least one), so
    /// a hostile count can't make us allocate.
    fn count(&mut self) -> Option<usize> {
        let n = self.u32()? as usize;
        (n <= self.0.len()).then_some(n)
    }

    fn release(&mut self) -> Option<Release> {
        Some(Release { major: self.u32()?, minor: self.u32()?, patch: self.u32()? })
    }

    fn version(&mut self) -> Option<PackageVersion> {
        let release = self.release()?;
        let len = self.u32()? as usize;
        let pre = core::str::from_utf8(self.take(len)?).ok()?.into();
        Some(PackageVersion { release, pre })
    }

    fn numbers(&mut self) -> Option<Vec<u16>> {
        let n = self.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let number = self.number()?;
            if out.last().is_some_and(|&last| last >= number) {
                return None;
            }
            out.push(number);
        }
        Some(out)
    }

    fn reason(&mut self) -> Option<Reason> {
        Reason::from_code(self.u8()?)
    }

    fn refusal(&mut self) -> Option<Refusal> {
        let reason = self.reason()?;
        let side = Side::from_code(self.u8()?)?;
        let protocol = if reason.per_protocol() { Some(self.number()?) } else { None };
        Some(Refusal { reason, side, protocol })
    }

    fn message(&mut self) -> Option<Message> {
        Some(match self.u8()? {
            HELLO => {
                let exchange = Exchange::from_code(self.u8()?)?;
                let package_id = self.hash()?;
                let version = self.version()?;
                let n = self.count()?;
                let mut encodings: Vec<Encoding> = Vec::with_capacity(n);
                for _ in 0..n {
                    let e = Encoding {
                        kind: EncodingKind::from_code(self.u8()?)?,
                        param_hash: self.hash()?,
                    };
                    if encodings.last().is_some_and(|last| *last >= e) {
                        return None;
                    }
                    encodings.push(e);
                }
                let n = self.count()?;
                let mut protocols: Vec<Protocol> = Vec::with_capacity(n);
                for _ in 0..n {
                    let number = self.number()?;
                    if protocols.last().is_some_and(|last| last.number >= number) {
                        return None;
                    }
                    protocols.push(Protocol {
                        number,
                        line: self.release()?,
                        shape_hash: self.hash()?,
                        functions: self.numbers()?,
                    });
                }
                Message::Hello(Hello { exchange, package_id, version, encodings, protocols })
            }
            ANSWER => Message::Answer(match self.u8()? {
                ACCEPT => {
                    let version = self.version()?;
                    let n = self.count()?;
                    let mut protocols = Vec::with_capacity(n);
                    for _ in 0..n {
                        let number = self.number()?;
                        let status = match self.u8()? {
                            s @ (ACCEPTED | ACCEPTED_SHAPE_DIFFERS) => Status::Accepted {
                                shape_differs: s == ACCEPTED_SHAPE_DIFFERS,
                                missing: self.numbers()?,
                            },
                            REFUSED => Status::Refused { reason: self.reason()? },
                            _ => return None,
                        };
                        protocols.push(Answered { number, status });
                    }
                    Answer::Accept { version, protocols }
                }
                REFUSE => Answer::Refuse(self.refusal()?),
                _ => return None,
            }),
            VERDICT => Message::Verdict(match self.u8()? {
                ACCEPT => {
                    let n = self.count()?;
                    let mut refused: Vec<RefusedProtocol> = Vec::with_capacity(n);
                    for _ in 0..n {
                        let r = RefusedProtocol { number: self.number()?, reason: self.reason()? };
                        if refused.last().is_some_and(|last| last.number >= r.number) {
                            return None;
                        }
                        refused.push(r);
                    }
                    Verdict::Accept { refused }
                }
                REFUSE => Verdict::Refuse(self.refusal()?),
                _ => return None,
            }),
            _ => return None,
        })
    }
}

// ── the outcome ────────────────────────────────────────────────────────────

/// What one of this end's protocols came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolState {
    /// Usable. `peer_lacks`: this end's functions the peer doesn't have;
    /// calls to them fail locally ([`Negotiated::check_call`]).
    Accepted { peer_lacks: Vec<u16> },
    /// Refused on its own (`scope = protocol`): every call to it fails
    /// locally.
    Refused { reason: Reason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolOutcome {
    pub number: u16,
    pub state: ProtocolState,
}

/// What a connection agreed: per protocol of this end's, whether it's
/// usable and which of its functions the peer lacks; and the protocols that
/// connected despite a shape differing at the same version
/// (`same_version_differs = warn`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiated {
    pub peer_version: PackageVersion,
    /// Ascending by number.
    pub protocols: Vec<ProtocolOutcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<u16>,
}

impl Negotiated {
    /// Whether this end may call `function` of `protocol`: `Ok` to send it,
    /// or [`RuntimeError::Unavailable`] when the peer lacks the function or
    /// the protocol was refused. Generated code calls this before it writes
    /// a request, so a call the peer can't take never reaches the wire.
    pub fn check_call(&self, protocol: u16, function: u16) -> Result<(), RuntimeError> {
        let unavailable = RuntimeError::Unavailable;
        let p = self
            .protocols
            .binary_search_by_key(&protocol, |p| p.number)
            .map_err(|_| unavailable)?;
        match &self.protocols[p].state {
            ProtocolState::Accepted { peer_lacks } if peer_lacks.binary_search(&function).is_err() => Ok(()),
            _ => Err(unavailable),
        }
    }

    /// Takes in the protocols the peer's verdict refused.
    fn apply(&mut self, refused: &[RefusedProtocol]) {
        for r in refused {
            if let Ok(i) = self.protocols.binary_search_by_key(&r.number, |p| p.number) {
                if matches!(self.protocols[i].state, ProtocolState::Accepted { .. }) {
                    self.protocols[i].state = ProtocolState::Refused { reason: r.reason };
                }
            }
        }
    }
}

/// One step's result: the message to send (if any), and what this end
/// concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub reply: Option<Message>,
    pub result: Result<Negotiated, Refusal>,
}

fn difference(a: &[u16], b: &[u16]) -> Vec<u16> {
    a.iter().copied().filter(|n| b.binary_search(n).is_err()).collect()
}

/// `comline@2` for one end, sans-IO: its parameters and its [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comline2 {
    pub params: Params,
    pub hello: Hello,
}

impl Comline2 {
    /// `hello` is put in canonical order, and its `exchange` set from
    /// `params`.
    pub fn new(params: Params, hello: Hello) -> Self {
        let mut hello = hello.canonical();
        hello.exchange = params.exchange;
        Self { params, hello }
    }

    /// The [`Hello`] this end sends.
    pub fn hello_message(&self) -> Message {
        Message::Hello(self.hello.clone())
    }

    fn refusal(&self, reason: Reason, side: Side, protocol: Option<u16>) -> Refusal {
        Refusal { reason, side, protocol }
    }

    /// The checks before any protocol: the exchange, the package, the
    /// encodings.
    fn check_connection(&self, peer: &Hello, me: Side) -> Result<(), Refusal> {
        let refuse = |reason| Err(self.refusal(reason, me, None));
        if peer.exchange != self.params.exchange {
            return refuse(Reason::ExchangeMismatch);
        }
        if peer.package_id != self.hello.package_id {
            return refuse(Reason::PackageMismatch);
        }
        if peer.encodings != self.hello.encodings {
            return refuse(Reason::EncodingMismatch);
        }
        Ok(())
    }

    /// Judge one protocol both ends have, knowing both hellos. `Ok(warn)`.
    fn judge_protocol(
        &self,
        me: Side,
        peer_version: &PackageVersion,
        mine: &Protocol,
        theirs: &Protocol,
    ) -> Result<bool, Reason> {
        if mine.line != theirs.line {
            return Err(Reason::LineMismatch);
        }
        let mut warn = false;
        if mine.shape_hash != theirs.shape_hash && *peer_version == self.hello.version {
            match self.params.same_version_differs {
                SameVersionDiffers::Refuse => return Err(Reason::SameVersionDiffers),
                SameVersionDiffers::Warn => warn = true,
            }
        }
        let (consumer, provider) = match me {
            Side::Initiator => (mine, theirs),
            Side::Responder => (theirs, mine),
        };
        if self.params.newer_consumer == NewerConsumer::Refuse
            && !difference(&consumer.functions, &provider.functions).is_empty()
        {
            return Err(Reason::NewerConsumer);
        }
        Ok(warn)
    }

    /// Apply `scope` to the per-protocol outcomes: a refusal of the whole
    /// connection, if any.
    fn scoped(&self, me: Side, protocols: &[(u16, Option<Reason>)]) -> Result<(), Refusal> {
        let refused = || protocols.iter().filter_map(|(n, r)| r.map(|r| (*n, r)));
        if self.params.scope == Scope::Package {
            if let Some((n, r)) = refused().find(|(_, r)| *r != Reason::UnknownProtocol) {
                return Err(self.refusal(r, me, Some(n)));
            }
        }
        if !protocols.is_empty() && protocols.iter().all(|(_, r)| r.is_some()) {
            let (n, r) = refused().next().unwrap();
            return Err(self.refusal(r, me, Some(n)));
        }
        Ok(())
    }

    /// Judge the peer's [`Hello`] knowing both: the responder under
    /// `offer`, and both ends under `symmetric`. Every protocol either end
    /// has is judged; one only one end has is `unknown_protocol`.
    fn judge(&self, me: Side, peer: &Hello) -> Result<(Negotiated, Vec<(u16, Option<Reason>, bool, Vec<u16>)>), Refusal> {
        self.check_connection(peer, me)?;
        let mut numbers: Vec<u16> = self
            .hello
            .protocols
            .iter()
            .chain(&peer.protocols)
            .map(|p| p.number)
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        // (number, refusal, shape differs, the peer's functions this end lacks)
        let mut judged = Vec::new();
        let mut negotiated = Negotiated {
            peer_version: peer.version.clone(),
            protocols: Vec::new(),
            warnings: Vec::new(),
        };
        for n in numbers {
            let (mine, theirs) = (self.hello.protocol(n), peer.protocol(n));
            let (Some(mine), Some(theirs)) = (mine, theirs) else {
                judged.push((n, Some(Reason::UnknownProtocol), false, Vec::new()));
                if mine.is_some() {
                    negotiated.protocols.push(ProtocolOutcome {
                        number: n,
                        state: ProtocolState::Refused { reason: Reason::UnknownProtocol },
                    });
                }
                continue;
            };
            let missing = difference(&theirs.functions, &mine.functions);
            let differs = mine.shape_hash != theirs.shape_hash;
            match self.judge_protocol(me, &peer.version, mine, theirs) {
                Ok(warn) => {
                    if warn {
                        negotiated.warnings.push(n);
                    }
                    judged.push((n, None, differs, missing));
                    negotiated.protocols.push(ProtocolOutcome {
                        number: n,
                        state: ProtocolState::Accepted {
                            peer_lacks: difference(&mine.functions, &theirs.functions),
                        },
                    });
                }
                Err(reason) => {
                    judged.push((n, Some(reason), differs, missing));
                    negotiated.protocols.push(ProtocolOutcome {
                        number: n,
                        state: ProtocolState::Refused { reason },
                    });
                }
            }
        }
        let outcomes: Vec<(u16, Option<Reason>)> = judged.iter().map(|j| (j.0, j.1)).collect();
        self.scoped(me, &outcomes)?;
        Ok((negotiated, judged))
    }

    fn peer_hello(bytes: &[u8]) -> Result<Hello, Reason> {
        match Message::decode(bytes)? {
            Message::Hello(h) => Ok(h),
            _ => Err(Reason::Malformed),
        }
    }

    /// `offer`, the responder: judge the initiator's [`Hello`] and answer.
    /// An accepted result still waits on the initiator's [`Verdict`]
    /// ([`finish`](Self::finish)).
    pub fn answer_offer(&self, offer: &[u8]) -> Step {
        let me = Side::Responder;
        let refuse = |r: Refusal| Step {
            reply: Some(Message::Answer(Answer::Refuse(r))),
            result: Err(r),
        };
        let peer = match Self::peer_hello(offer) {
            Ok(peer) => peer,
            Err(reason) => return refuse(self.refusal(reason, me, None)),
        };
        if self.params.exchange != Exchange::Offer || peer.exchange != Exchange::Offer {
            return refuse(self.refusal(Reason::ExchangeMismatch, me, None));
        }
        match self.judge(me, &peer) {
            Err(r) => refuse(r),
            Ok((negotiated, judged)) => {
                // Answer every offered protocol, in the offer's order.
                let protocols = peer
                    .protocols
                    .iter()
                    .map(|p| {
                        let j = judged.iter().find(|j| j.0 == p.number).unwrap();
                        Answered {
                            number: p.number,
                            status: match j.1 {
                                Some(reason) => Status::Refused { reason },
                                None => Status::Accepted { shape_differs: j.2, missing: j.3.clone() },
                            },
                        }
                    })
                    .collect();
                Step {
                    reply: Some(Message::Answer(Answer::Accept {
                        version: self.hello.version.clone(),
                        protocols,
                    })),
                    result: Ok(negotiated),
                }
            }
        }
    }

    /// `offer`, the initiator: judge the responder's [`Answer`] by this
    /// end's own policy, and give the [`Verdict`] to send (none when the
    /// responder refused).
    pub fn conclude_answer(&self, answer: &[u8]) -> Step {
        let me = Side::Initiator;
        let refuse = |r: Refusal| Step {
            reply: Some(Message::Verdict(Verdict::Refuse(r))),
            result: Err(r),
        };
        let (version, answered) = match Message::decode(answer) {
            Ok(Message::Answer(Answer::Refuse(r))) => return Step { reply: None, result: Err(r) },
            Ok(Message::Answer(Answer::Accept { version, protocols })) => (version, protocols),
            // A symmetric responder sends a hello where an answer belongs.
            Ok(Message::Hello(_)) => return refuse(self.refusal(Reason::ExchangeMismatch, me, None)),
            Ok(Message::Verdict(_)) => return refuse(self.refusal(Reason::Malformed, me, None)),
            Err(reason) => return refuse(self.refusal(reason, me, None)),
        };
        let offered: Vec<u16> = self.hello.protocols.iter().map(|p| p.number).collect();
        if answered.iter().map(|a| a.number).collect::<Vec<_>>() != offered {
            return refuse(self.refusal(Reason::Malformed, me, None));
        }
        let mut negotiated =
            Negotiated { peer_version: version.clone(), protocols: Vec::new(), warnings: Vec::new() };
        let mut outcomes = Vec::new();
        let mut own = Vec::new();
        for (mine, a) in self.hello.protocols.iter().zip(&answered) {
            let state = match &a.status {
                Status::Refused { reason } => Err(*reason),
                Status::Accepted { shape_differs, missing } => {
                    if missing.iter().any(|n| mine.functions.binary_search(n).is_err()) {
                        return refuse(self.refusal(Reason::Malformed, me, None));
                    }
                    let same = *shape_differs && version == self.hello.version;
                    if same && self.params.same_version_differs == SameVersionDiffers::Refuse {
                        own.push(RefusedProtocol { number: a.number, reason: Reason::SameVersionDiffers });
                        Err(Reason::SameVersionDiffers)
                    } else if !missing.is_empty() && self.params.newer_consumer == NewerConsumer::Refuse {
                        own.push(RefusedProtocol { number: a.number, reason: Reason::NewerConsumer });
                        Err(Reason::NewerConsumer)
                    } else {
                        if same {
                            negotiated.warnings.push(a.number);
                        }
                        Ok(missing.clone())
                    }
                }
            };
            outcomes.push((a.number, state.as_ref().err().copied()));
            negotiated.protocols.push(ProtocolOutcome {
                number: a.number,
                state: match state {
                    Ok(peer_lacks) => ProtocolState::Accepted { peer_lacks },
                    Err(reason) => ProtocolState::Refused { reason },
                },
            });
        }
        if let Err(r) = self.scoped(me, &outcomes) {
            return refuse(r);
        }
        Step {
            reply: Some(Message::Verdict(Verdict::Accept { refused: own })),
            result: Ok(negotiated),
        }
    }

    /// `symmetric`, either end: judge the peer's [`Hello`] and give this
    /// end's [`Verdict`]. An accepted result still waits on the peer's
    /// ([`finish`](Self::finish)).
    pub fn judge_hello(&self, me: Side, peer: &[u8]) -> Step {
        let refuse = |r: Refusal| Step {
            reply: Some(Message::Verdict(Verdict::Refuse(r))),
            result: Err(r),
        };
        let peer = match Message::decode(peer) {
            Ok(Message::Hello(peer)) => peer,
            // An offer responder refusing (our hello isn't an offer), or a
            // peer that refused before judging: take its refusal.
            Ok(Message::Answer(Answer::Refuse(r)) | Message::Verdict(Verdict::Refuse(r))) => {
                return Step { reply: None, result: Err(r) }
            }
            Ok(_) => return refuse(self.refusal(Reason::Malformed, me, None)),
            Err(reason) => return refuse(self.refusal(reason, me, None)),
        };
        if self.params.exchange != Exchange::Symmetric {
            return refuse(self.refusal(Reason::ExchangeMismatch, me, None));
        }
        match self.judge(me, &peer) {
            Err(r) => refuse(r),
            Ok((negotiated, judged)) => {
                let refused = judged
                    .iter()
                    .filter_map(|j| j.1.map(|reason| RefusedProtocol { number: j.0, reason }))
                    .collect();
                Step {
                    reply: Some(Message::Verdict(Verdict::Accept { refused })),
                    result: Ok(negotiated),
                }
            }
        }
    }

    /// Take in the peer's [`Verdict`]: its refusal, or the protocols it
    /// refused. A message that isn't a verdict is `malformed`, refused by
    /// `me`.
    pub fn finish(me: Side, mut negotiated: Negotiated, verdict: &[u8]) -> Result<Negotiated, Refusal> {
        match Message::decode(verdict) {
            Ok(Message::Verdict(Verdict::Accept { refused })) => {
                negotiated.apply(&refused);
                Ok(negotiated)
            }
            Ok(Message::Verdict(Verdict::Refuse(r))) => Err(r),
            Ok(_) => Err(Refusal { reason: Reason::Malformed, side: me, protocol: None }),
            Err(reason) => Err(Refusal { reason, side: me, protocol: None }),
        }
    }
}

// ── over a transport ───────────────────────────────────────────────────────

/// `comline@2` as a [`Handshake`]: [`Comline2`] run over a transport. After
/// it succeeds, [`negotiated`](Self::negotiated) holds what the connection
/// agreed; after it refuses, [`refusal`](Self::refusal) says why, and the
/// error is [`RuntimeError::Refused`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comline2Handshake {
    pub local: Comline2,
    negotiated: Option<Negotiated>,
    refusal: Option<Refusal>,
}

impl Comline2Handshake {
    pub fn new(params: Params, hello: Hello) -> Self {
        Self { local: Comline2::new(params, hello), negotiated: None, refusal: None }
    }

    pub fn negotiated(&self) -> Option<&Negotiated> {
        self.negotiated.as_ref()
    }

    pub fn refusal(&self) -> Option<&Refusal> {
        self.refusal.as_ref()
    }

    fn settle(&mut self, result: Result<Negotiated, Refusal>) -> Result<(), RuntimeError> {
        match result {
            Ok(n) => {
                self.negotiated = Some(n);
                Ok(())
            }
            Err(r) => {
                self.refusal = Some(r);
                Err(r.error())
            }
        }
    }
}

/// Sends `message` when there is one.
fn send<T: Transport>(transport: &mut T, message: &Option<Message>) -> Result<(), RuntimeError> {
    match message {
        Some(m) => transport.send(&m.to_bytes()),
        None => Ok(()),
    }
}

impl Handshake for Comline2Handshake {
    fn name(&self) -> &str {
        HANDSHAKE_COMLINE2
    }

    fn initiate<T: Transport>(&mut self, transport: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        let me = Side::Initiator;
        transport.send(&self.local.hello_message().to_bytes())?;
        let mut buf = Vec::new();
        transport.recv(&mut buf)?;
        let step = match self.local.params.exchange {
            Exchange::Offer => self.local.conclude_answer(&buf),
            Exchange::Symmetric => self.local.judge_hello(me, &buf),
        };
        // Our refusal is sent best-effort: the peer may already have gone,
        // having refused first. Under `symmetric` its verdict may still be
        // there to read when ours couldn't be sent.
        let sent = send(transport, &step.reply);
        let result = match (self.local.params.exchange, step.result) {
            (Exchange::Symmetric, Ok(n)) => {
                buf.clear();
                transport.recv(&mut buf).map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            (_, Ok(n)) => {
                sent?;
                Ok(n)
            }
            (_, result) => result,
        };
        self.settle(result)
    }

    fn respond<T: Transport>(&mut self, transport: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        let me = Side::Responder;
        let mut buf = Vec::new();
        let step = match self.local.params.exchange {
            Exchange::Offer => {
                transport.recv(&mut buf)?;
                self.local.answer_offer(&buf)
            }
            Exchange::Symmetric => {
                transport.send(&self.local.hello_message().to_bytes())?;
                transport.recv(&mut buf)?;
                self.local.judge_hello(me, &buf)
            }
        };
        let sent = send(transport, &step.reply);
        let result = match step.result {
            Ok(n) => {
                buf.clear();
                transport.recv(&mut buf).map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            Err(r) => Err(r),
        };
        self.settle(result)
    }
}

#[cfg(feature = "async")]
async fn send_async<T: AsyncTransport>(transport: &mut T, message: &Option<Message>) -> Result<(), RuntimeError> {
    match message {
        Some(m) => transport.send(m.to_bytes()).await,
        None => Ok(()),
    }
}

#[cfg(feature = "async")]
async fn recv_async<T: AsyncTransport>(transport: &mut T) -> Result<Vec<u8>, RuntimeError> {
    transport.next().await.ok_or(RuntimeError::Transport)?
}

#[cfg(feature = "async")]
impl AsyncHandshake for Comline2Handshake {
    fn name(&self) -> &str {
        HANDSHAKE_COMLINE2
    }

    async fn initiate<T: AsyncTransport>(&mut self, transport: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        let me = Side::Initiator;
        transport.send(self.local.hello_message().to_bytes()).await?;
        let buf = recv_async(transport).await?;
        let step = match self.local.params.exchange {
            Exchange::Offer => self.local.conclude_answer(&buf),
            Exchange::Symmetric => self.local.judge_hello(me, &buf),
        };
        let sent = send_async(transport, &step.reply).await;
        let result = match (self.local.params.exchange, step.result) {
            (Exchange::Symmetric, Ok(n)) => {
                let buf = recv_async(transport).await.map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            (_, Ok(n)) => {
                sent?;
                Ok(n)
            }
            (_, result) => result,
        };
        self.settle(result)
    }

    async fn respond<T: AsyncTransport>(&mut self, transport: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        let me = Side::Responder;
        let step = match self.local.params.exchange {
            Exchange::Offer => self.local.answer_offer(&recv_async(transport).await?),
            Exchange::Symmetric => {
                transport.send(self.local.hello_message().to_bytes()).await?;
                self.local.judge_hello(me, &recv_async(transport).await?)
            }
        };
        let sent = send_async(transport, &step.reply).await;
        let result = match step.result {
            Ok(n) => {
                let buf = recv_async(transport).await.map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            Err(r) => Err(r),
        };
        self.settle(result)
    }
}
