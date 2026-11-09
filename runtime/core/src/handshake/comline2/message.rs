use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::contract::{
    put_varint, take_varint, BufMut, Reason, RuntimeError, Side, HANDSHAKE_MAGIC,
};

use super::facts::{Encoding, EncodingKind, Hash128, Hello, PackageVersion, Protocol, Release};
use super::Exchange;

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
        RuntimeError::Refused {
            reason: self.reason,
            side: self.side,
        }
    }
}

/// The responder's judgement of one offered protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Accepted. `shape_differs`: the shape hashes differ (which matters to
    /// the initiator's `same_version_differs` when the versions are equal);
    /// `missing`: the offered functions the responder lacks.
    Accepted {
        shape_differs: bool,
        missing: Vec<u16>,
    },
    Refused {
        reason: Reason,
    },
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
    Accept {
        version: PackageVersion,
        protocols: Vec<Answered>,
    },
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
        out.put_slice(&HANDSHAKE_MAGIC);
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
                                Status::Accepted {
                                    shape_differs,
                                    missing,
                                } => {
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
        Some(Release {
            major: self.u32()?,
            minor: self.u32()?,
            patch: self.u32()?,
        })
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
        let protocol = if reason.per_protocol() {
            Some(self.number()?)
        } else {
            None
        };
        Some(Refusal {
            reason,
            side,
            protocol,
        })
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
                Message::Hello(Hello {
                    exchange,
                    package_id,
                    version,
                    encodings,
                    protocols,
                })
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
                            REFUSED => Status::Refused {
                                reason: self.reason()?,
                            },
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
                        let r = RefusedProtocol {
                            number: self.number()?,
                            reason: self.reason()?,
                        };
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
