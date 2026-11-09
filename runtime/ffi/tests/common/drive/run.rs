use std::collections::{BTreeSet, VecDeque};

use super::abi::{Encoded, Fed, Ffi};
use super::reference::{Fr, Reference};
use super::status::{bit, statuses};
use super::{Cap, Config, Hello, Op, Ours, PeerEnvelope, Theirs, PLENTY};
use comline_runtime::contract::{
    ConnectionFraming, Datagram2Framing, Datagram3Framing, Envelope, HandshakeFrame,
    FRAMING_DATAGRAM2, FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime_ffi::*;

/// One of this side's encodes, its arguments resolved.
#[derive(Debug, Clone, Copy)]
enum Encode<'a> {
    Start,
    Request {
        protocol: u16,
        call_id: u16,
        name: &'a [u8],
        expects_reply: u8,
        body: &'a [u8],
    },
    Reply {
        protocol: u16,
        request_id: u64,
        envelope: u8,
        error_id: u16,
        body: &'a [u8],
    },
}

pub(super) struct Run {
    ffi: Ffi,
    reference: Option<Reference>,
    fr: Fr,
    stream: bool,
    unordered: bool,
    handshake: bool,
    max_frame: usize,
    max_waiting: usize,
    /// What the peer's handshake names when it agrees.
    ir_hash: u64,
    wire_format: String,
    framing: String,

    /// Stream mode: bytes the peer sent that aren't delivered yet.
    inbound: Vec<u8>,
    /// Message mode: messages the peer sent that aren't delivered yet.
    messages: VecDeque<Vec<u8>>,
    /// Stream mode: delivered bytes no `feed` consumed yet.
    read: Vec<u8>,

    // What the calls so far say, by the ABI's rules.
    sent: bool,
    received: bool,
    closed: bool,
    next_id: u64,
    /// Waiting ids, innermost last.
    waiting: Vec<u64>,
    spent: BTreeSet<u64>,
    /// The peer's request ids, as they came.
    theirs: Vec<u64>,
}

impl Run {
    /// A run on a handle `config` made, so `config` is valid.
    pub(super) fn new(config: &Config, ffi: Ffi, reference: Option<Reference>) -> Self {
        let framing = String::from_utf8_lossy(&config.framing).into_owned();
        let handshake = config.handshake == COMLINE_HANDSHAKE_COMLINE;
        Self {
            ffi,
            reference,
            fr: match framing.as_str() {
                FRAMING_JSONRPC => Fr::JsonRpc,
                FRAMING_DATAGRAM3 => Fr::Datagram3,
                _ => Fr::Datagram2,
            },
            stream: config.stream == 1,
            unordered: config.unordered == 1,
            handshake,
            max_frame: match config.max_frame {
                0 => COMLINE_DEFAULT_MAX_FRAME as usize,
                n => n as usize,
            },
            max_waiting: config.max_waiting as usize,
            ir_hash: config.ir_hash,
            wire_format: String::from_utf8_lossy(&config.wire_format).into_owned(),
            framing,
            inbound: Vec::new(),
            messages: VecDeque::new(),
            read: Vec::new(),
            sent: !handshake,
            received: !handshake,
            closed: false,
            next_id: 0,
            waiting: Vec::new(),
            spent: BTreeSet::new(),
            theirs: Vec::new(),
        }
    }

    fn open(&self) -> bool {
        self.sent && self.received
    }

    pub(super) fn step(&mut self, op: &Op) -> Result<(), String> {
        match op {
            Op::Start(cap) => self.encode(Encode::Start, *cap),
            Op::Call {
                protocol,
                call_id,
                name,
                expects_reply,
                body,
                cap,
            } => self.encode(
                Encode::Request {
                    protocol: *protocol,
                    call_id: *call_id,
                    name,
                    expects_reply: *expects_reply,
                    body,
                },
                *cap,
            ),
            Op::Answer {
                protocol,
                to,
                envelope,
                error_id,
                body,
                cap,
            } => {
                let request_id = match *to {
                    Theirs::Received(i) if !self.theirs.is_empty() => {
                        self.theirs[i as usize % self.theirs.len()]
                    }
                    Theirs::Received(_) => 0,
                    Theirs::Raw(id) => id,
                };
                self.encode(
                    Encode::Reply {
                        protocol: *protocol,
                        request_id,
                        envelope: *envelope,
                        error_id: *error_id,
                        body,
                    },
                    *cap,
                )
            }
            Op::Abandon(to) => {
                let id = self.resolve(*to);
                let status = self.ffi.abandon(id)?;
                let want = if self.closed {
                    COMLINE_ERR_STATE
                } else {
                    COMLINE_OK
                };
                ensure!(status == want, "abandon: {status}, not {want}");
                if let Some(reference) = &mut self.reference {
                    let theirs = reference.abandon(id);
                    ensure!(
                        status == theirs,
                        "abandon: {status}, the reference {theirs}"
                    );
                }
                if let Some(at) = self.waiting.iter().position(|&w| w == id) {
                    self.waiting.remove(at);
                    self.spent.insert(id);
                }
                Ok(())
            }
            Op::IsWaiting => {
                let got = self.ffi.is_waiting()?;
                let want = if self.closed {
                    COMLINE_ERR_STATE
                } else {
                    COMLINE_OK
                };
                ensure!(got.0 == want, "is_waiting: {}, not {want}", got.0);
                if got.0 == COMLINE_OK {
                    let waiting = u8::from(!self.waiting.is_empty());
                    ensure!(
                        got.1 == waiting,
                        "is_waiting: {} with {:?} waiting",
                        got.1,
                        self.waiting
                    );
                }
                if let Some(reference) = &self.reference {
                    let theirs = reference.is_waiting();
                    ensure!(
                        got.0 == theirs.0 && (got.0 != COMLINE_OK || got.1 == theirs.1),
                        "is_waiting: {got:?}, the reference {theirs:?}"
                    );
                }
                Ok(())
            }
            Op::PeerHello {
                hello,
                capabilities,
            } => {
                let (ir_hash, mut wire_format, mut framing) =
                    (self.ir_hash, self.wire_format.clone(), self.framing.clone());
                let ir_hash = match hello {
                    Hello::IrHash => ir_hash ^ 1,
                    _ => ir_hash,
                };
                match hello {
                    Hello::WireFormat => wire_format.push('x'),
                    Hello::Framing => {
                        framing = if self.fr == Fr::JsonRpc {
                            FRAMING_DATAGRAM2
                        } else {
                            FRAMING_JSONRPC
                        }
                        .into()
                    }
                    _ => {}
                }
                let mut frame = Vec::new();
                HandshakeFrame::new(ir_hash, &wire_format, &framing, *capabilities)
                    .encode(&mut frame);
                match hello {
                    Hello::Short(n) => frame.truncate(*n as usize % frame.len()),
                    Hello::Trailing(n) => frame.extend((0..*n).map(|i| i.wrapping_mul(37))),
                    _ => {}
                }
                self.send(frame);
                Ok(())
            }
            Op::PeerRequest {
                protocol,
                call_id,
                name,
                request_id,
                one_way,
                body,
            } => {
                let mut frame = Vec::new();
                framed!(self.fr, |f| f.encode_request_body(
                    *protocol,
                    *call_id,
                    name,
                    *request_id,
                    *one_way,
                    body,
                    &mut frame
                ));
                self.send(frame);
                Ok(())
            }
            Op::PeerReply {
                to,
                protocol,
                envelope,
                body,
            } => {
                let id = self.resolve(*to);
                let envelope = match envelope {
                    PeerEnvelope::Ok => Envelope::Ok(body),
                    PeerEnvelope::Err(id) => Envelope::Err { id: *id, body },
                    PeerEnvelope::Fault => Envelope::Fault,
                };
                let mut frame = Vec::new();
                framed!(self.fr, |f| f
                    .encode_reply(*protocol, id, &envelope, &mut frame));
                self.send(frame);
                Ok(())
            }
            Op::PeerRaw { framed, bytes } => {
                if self.stream && !framed {
                    self.inbound.extend_from_slice(bytes);
                } else {
                    self.send(bytes.clone());
                }
                Ok(())
            }
            Op::Deliver(n) => self.deliver(*n as usize),
            Op::Corrupt { at, xor } => {
                let bytes = if self.stream {
                    Some(&mut self.inbound)
                } else {
                    self.messages.back_mut()
                };
                if let Some(bytes) = bytes.filter(|b| !b.is_empty()) {
                    let len = bytes.len();
                    bytes[len - 1 - *at as usize % len] ^= (*xor).max(1);
                }
                Ok(())
            }
        }
    }

    /// The id `to` names now, as `state_model.rs` resolves a `ReplyTo`.
    fn resolve(&self, to: Ours) -> u64 {
        match to {
            Ours::Waiting(i) if !self.waiting.is_empty() => {
                self.waiting[self.waiting.len() - 1 - i as usize % self.waiting.len()]
            }
            Ours::Waiting(i) | Ours::Spent(i) if !self.spent.is_empty() => *self
                .spent
                .iter()
                .nth(i as usize % self.spent.len())
                .expect("in range"),
            Ours::Never(offset) => self.next_id.wrapping_add(offset),
            Ours::Raw(id) => id,
            Ours::Waiting(_) | Ours::Spent(_) => self.next_id,
        }
    }

    /// The peer sends `frame`.
    fn send(&mut self, frame: Vec<u8>) {
        if self.stream {
            self.inbound
                .extend_from_slice(&(frame.len() as u32).to_le_bytes());
            self.inbound.extend_from_slice(&frame);
        } else {
            self.messages.push_back(frame);
        }
    }

    // ── encoding ───────────────────────────────────────────────────────────

    fn encode(&mut self, what: Encode<'_>, cap: Cap) -> Result<(), String> {
        let first = match cap {
            Cap::Plenty => PLENTY,
            Cap::Fixed(n) => n as usize,
            Cap::Exact | Cap::Short(_) => 0,
        };
        let got = self.encode_both(what, first)?;
        if got.status != COMLINE_BUFFER_TOO_SMALL {
            return Ok(());
        }
        let needed = got.written;
        match cap {
            Cap::Exact => {
                let again = self.encode_both(what, needed)?;
                ensure!(
                    (again.status, again.written) == (COMLINE_OK, needed),
                    "BUFFER_TOO_SMALL needing {needed}, then {} writing {} with that room",
                    again.status,
                    again.written
                );
            }
            Cap::Short(k) => {
                self.encode_both(what, needed - 1 - k as usize % needed)?;
            }
            Cap::Plenty | Cap::Fixed(_) => {}
        }
        Ok(())
    }

    fn encode_both(&mut self, what: Encode<'_>, cap: usize) -> Result<Encoded, String> {
        let got = match what {
            Encode::Start => self.ffi.start(cap)?,
            Encode::Request {
                protocol,
                call_id,
                name,
                expects_reply,
                body,
            } => self
                .ffi
                .request(protocol, call_id, name, expects_reply, body, cap)?,
            Encode::Reply {
                protocol,
                request_id,
                envelope,
                error_id,
                body,
            } => self
                .ffi
                .reply(protocol, request_id, envelope, error_id, body, cap)?,
        };
        self.check_encoded(what, &got)
            .map_err(|e| format!("cap {cap}: {e}: {got:?}"))?;
        if let Some(reference) = &mut self.reference {
            let want = match what {
                Encode::Start => reference.start(cap),
                Encode::Request {
                    protocol,
                    call_id,
                    name,
                    expects_reply,
                    body,
                } => reference.request(protocol, call_id, name, expects_reply, body, cap)?,
                Encode::Reply {
                    protocol,
                    request_id,
                    envelope,
                    error_id,
                    body,
                } => reference.reply(protocol, request_id, envelope, error_id, body, cap),
            };
            same_encoded(what, &got, &want).map_err(|e| format!("cap {cap}: {e}"))?;
        }
        if got.status == COMLINE_OK {
            match what {
                Encode::Start => self.sent = true,
                Encode::Request { expects_reply, .. } => {
                    self.next_id = got.request_id.wrapping_add(1);
                    if expects_reply == 1 {
                        self.waiting.push(got.request_id);
                    } else {
                        self.spent.insert(got.request_id);
                    }
                }
                Encode::Reply { .. } => {}
            }
        }
        Ok(got)
    }

    /// The ABI's rules for one encode.
    fn check_encoded(&self, what: Encode<'_>, got: &Encoded) -> Result<(), String> {
        let status = got.status;
        if self.closed {
            ensure!(status == COMLINE_ERR_STATE, "a closed connection encoded");
            return Ok(());
        }
        let bad_argument = match what {
            Encode::Start => false,
            Encode::Request {
                name,
                expects_reply,
                ..
            } => std::str::from_utf8(name).is_err() || expects_reply > 1,
            Encode::Reply { envelope, body, .. } => {
                envelope > COMLINE_ENVELOPE_FAULT
                    || (envelope == COMLINE_ENVELOPE_FAULT && !body.is_empty())
            }
        };
        if bad_argument {
            ensure!(
                status == COMLINE_ERR_INVALID_ARGUMENT
                    || (status == COMLINE_ERR_STATE && !self.open()),
                "a bad argument was taken"
            );
            return Ok(());
        }
        ensure!(
            status != COMLINE_ERR_INVALID_ARGUMENT,
            "a good argument was refused"
        );
        let prefix = if self.stream { 4 } else { 0 };
        match what {
            Encode::Start if !self.handshake => {
                ensure!(
                    (status, got.written) == (COMLINE_OK, 0),
                    "start without a handshake wrote"
                );
                return Ok(());
            }
            Encode::Start if self.sent => {
                ensure!(status == COMLINE_ERR_STATE, "a second start");
                return Ok(());
            }
            Encode::Start => {
                let len = COMLINE_HANDSHAKE_LEN as usize;
                ensure!(
                    (status == COMLINE_ERR_FRAME_TOO_LARGE) == (len > self.max_frame),
                    "the handshake frame against max_frame {}",
                    self.max_frame
                );
                if matches!(status, COMLINE_OK | COMLINE_BUFFER_TOO_SMALL) {
                    ensure!(got.written == prefix + len, "the handshake frame's size");
                }
            }
            Encode::Request { .. } | Encode::Reply { .. } if !self.open() => {
                ensure!(
                    status == COMLINE_ERR_STATE,
                    "an encode before the handshake"
                );
                return Ok(());
            }
            Encode::Request { expects_reply, .. } => {
                ensure!(status != COMLINE_ERR_STATE, "STATE on an open connection");
                let busy = expects_reply == 1 && self.waiting.len() >= self.max_waiting;
                ensure!(
                    (status == COMLINE_ERR_BUSY) == busy,
                    "BUSY with {} of {} waiting",
                    self.waiting.len(),
                    self.max_waiting
                );
                if status == COMLINE_OK {
                    ensure!(
                        got.request_id == self.next_id,
                        "request id {}, not {}",
                        got.request_id,
                        self.next_id
                    );
                }
            }
            Encode::Reply { .. } => {
                ensure!(status != COMLINE_ERR_STATE, "STATE on an open connection");
            }
        }
        if status == COMLINE_OK {
            let bytes = &got.bytes;
            ensure!(bytes.len() >= prefix, "a frame shorter than its prefix");
            let frame = bytes.len() - prefix;
            if self.stream {
                ensure!(
                    bytes[..4] == (frame as u32).to_le_bytes(),
                    "a prefix that isn't the frame's length"
                );
            }
            ensure!(frame <= self.max_frame, "a frame over max_frame");
        }
        Ok(())
    }

    // ── feeding ────────────────────────────────────────────────────────────

    fn deliver(&mut self, n: usize) -> Result<(), String> {
        if !self.stream {
            let Some(mut message) = self.messages.pop_front() else {
                return Ok(());
            };
            if n != 0 {
                message.truncate(n);
            }
            self.feed_both(&message)?;
            return Ok(());
        }
        let take = if n == 0 {
            self.inbound.len()
        } else {
            n.min(self.inbound.len())
        };
        self.read.extend(self.inbound.drain(..take));
        // Feed until a call consumes nothing (it needs more, or refused).
        loop {
            let read = std::mem::take(&mut self.read);
            let fed = self.feed_both(&read);
            self.read = read;
            let fed = fed?;
            let consumed = match fed.status {
                COMLINE_OK
                | COMLINE_ERR_FRAMING
                | COMLINE_ERR_PEER_REFUSED
                | COMLINE_ERR_HANDSHAKE => fed.consumed,
                _ => 0,
            };
            if consumed == 0 {
                return Ok(());
            }
            self.read.drain(..consumed);
            if self.read.is_empty() {
                return Ok(());
            }
        }
    }

    fn feed_both(&mut self, input: &[u8]) -> Result<Fed, String> {
        let fed = self.ffi.feed(input)?;
        self.check_fed(input, &fed)
            .map_err(|e| format!("feed of {} bytes: {e}: {fed:?}", input.len()))?;
        if let Some(reference) = &mut self.reference {
            let want = reference.feed(input)?;
            same_fed(&fed, &want)
                .map_err(|e| format!("feed of {:02x?}: {e}", &input[..input.len().min(256)]))?;
        }
        match fed.status {
            COMLINE_ERR_HANDSHAKE => self.closed = true,
            COMLINE_ERR_FRAME_TOO_LARGE if self.stream => self.closed = true,
            COMLINE_OK => match fed.ev.kind {
                COMLINE_EVENT_HANDSHAKE_OK => self.received = true,
                COMLINE_EVENT_REQUEST => self.theirs.push(fed.ev.request_id),
                COMLINE_EVENT_REPLY => {
                    let id = fed.ev.request_id;
                    self.waiting.retain(|&w| w != id);
                    self.spent.insert(id);
                }
                _ => {}
            },
            _ => {}
        }
        Ok(fed)
    }

    /// The ABI's rules for one `feed`, from the input and what came before.
    fn check_fed(&self, input: &[u8], fed: &Fed) -> Result<(), String> {
        let status = fed.status;
        if self.closed {
            ensure!(status == COMLINE_ERR_STATE, "a closed connection was fed");
            return Ok(());
        }
        ensure!(
            !matches!(status, COMLINE_ERR_STATE | COMLINE_ERR_INVALID_ARGUMENT),
            "an open connection refused a feed"
        );
        let frame = if self.stream {
            let Some(prefix) = input.first_chunk::<4>() else {
                ensure!(
                    (status, fed.consumed) == (COMLINE_NEED_MORE, 0)
                        && fed.ev == comline_event::default(),
                    "no whole prefix, yet not NEED_MORE with a size of 0"
                );
                return Ok(());
            };
            let len = u32::from_le_bytes(*prefix) as usize;
            if len > self.max_frame {
                ensure!(
                    (status, fed.consumed) == (COMLINE_ERR_FRAME_TOO_LARGE, 0),
                    "a prefix over max_frame {} taken",
                    self.max_frame
                );
                return Ok(());
            }
            if 4 + len > input.len() {
                let need = comline_event {
                    frame: comline_span {
                        offset: 0,
                        len: (4 + len) as u32,
                    },
                    ..Default::default()
                };
                ensure!(
                    (status, fed.consumed) == (COMLINE_NEED_MORE, 0) && fed.ev == need,
                    "a partial frame, yet not NEED_MORE with its size {}",
                    4 + len
                );
                return Ok(());
            }
            comline_span {
                offset: 4,
                len: len as u32,
            }
        } else {
            if input.len() > self.max_frame {
                ensure!(
                    (status, fed.consumed) == (COMLINE_ERR_FRAME_TOO_LARGE, 0),
                    "a message over max_frame {} taken",
                    self.max_frame
                );
                return Ok(());
            }
            comline_span {
                offset: 0,
                len: input.len() as u32,
            }
        };
        let end = (frame.offset + frame.len) as usize;
        ensure!(
            fed.consumed == end,
            "consumed {}, the frame ends at {end}",
            fed.consumed
        );
        match status {
            COMLINE_OK => {
                ensure!(fed.ev.frame == frame, "the frame's span");
                if self.received {
                    ensure!(
                        fed.ev.kind != COMLINE_EVENT_HANDSHAKE_OK,
                        "a second handshake"
                    );
                } else {
                    ensure!(
                        fed.ev.kind == COMLINE_EVENT_HANDSHAKE_OK,
                        "the first frame isn't the handshake"
                    );
                }
                check_event(&fed.ev, self.fr)?;
                if fed.ev.kind == COMLINE_EVENT_REPLY {
                    let id = fed.ev.request_id;
                    let at = self.waiting.iter().position(|&w| w == id);
                    ensure!(
                        at.is_some(),
                        "a reply to {id}, with {:?} waiting",
                        self.waiting
                    );
                    ensure!(
                        self.unordered || at == Some(self.waiting.len() - 1),
                        "an ordered reply to {id}, not the innermost of {:?}",
                        self.waiting
                    );
                }
            }
            COMLINE_ERR_HANDSHAKE => {
                ensure!(!self.received, "a handshake error after the handshake")
            }
            COMLINE_ERR_FRAMING | COMLINE_ERR_PEER_REFUSED => {
                ensure!(self.received, "a framing error in the handshake")
            }
            _ => return Err("a whole frame within max_frame, refused".into()),
        }
        Ok(())
    }
}

/// Whether `inner` lies inside `outer`.
pub(super) fn inside(inner: comline_span, outer: comline_span) -> bool {
    let end = |s: comline_span| u64::from(s.offset) + u64::from(s.len);
    inner.offset >= outer.offset && end(inner) <= end(outer)
}

/// The fields valid for an event's kind and flags, and nothing else.
fn check_event(ev: &comline_event, fr: Fr) -> Result<(), String> {
    let none = comline_span::default();
    ensure!(
        ev._reserved == 0 && ev._reserved2 == 0,
        "reserved fields set"
    );
    // A span the event carries lies inside the frame; one it doesn't is 0.
    let has_name = ev.kind == COMLINE_EVENT_REQUEST && ev.flags & COMLINE_EVENT_CALL_BY_NAME != 0;
    let has_body = matches!(ev.kind, COMLINE_EVENT_REQUEST | COMLINE_EVENT_REPLY)
        && ev.envelope != COMLINE_ENVELOPE_FAULT
        && ev.flags & COMLINE_EVENT_BODY_ABSENT == 0;
    for (span, carried, what) in [
        (ev.call_name, has_name, "name"),
        (ev.body, has_body, "body"),
    ] {
        if carried {
            ensure!(inside(span, ev.frame), "the {what} span outside the frame");
        } else {
            ensure!(span == none, "a {what} span the event doesn't carry");
        }
    }
    let jsonrpc = fr == Fr::JsonRpc;
    match ev.kind {
        COMLINE_EVENT_HANDSHAKE_OK => ensure!(
            ev.flags == 0
                && (ev.protocol, ev.call_id, ev.envelope, ev.error_id) == (0, 0, 0, 0)
                && (ev.call_name, ev.body, ev.request_id) == (none, none, 0),
            "a handshake event with request or reply fields"
        ),
        COMLINE_EVENT_REQUEST => {
            ensure!(
                (ev.envelope, ev.error_id, ev.capabilities) == (0, 0, 0),
                "a request with reply fields"
            );
            let by_name = ev.flags & COMLINE_EVENT_CALL_BY_NAME != 0;
            let has_protocol = ev.flags & COMLINE_EVENT_HAS_PROTOCOL != 0;
            if jsonrpc {
                ensure!(
                    by_name && !has_protocol && ev.flags & !0x1f == 0,
                    "a jsonrpc@1 request's flags {:#x}",
                    ev.flags
                );
            } else {
                ensure!(
                    ev.flags == COMLINE_EVENT_HAS_PROTOCOL,
                    "a datagram request's flags {:#x}",
                    ev.flags
                );
            }
            ensure!(has_protocol || ev.protocol == 0, "a protocol not flagged");
            ensure!(!by_name || ev.call_id == 0, "a call id beside a name");
            ensure!(by_name || ev.call_name == none, "a name not flagged");
            ensure!(
                by_name || ev.flags & COMLINE_EVENT_NAME_ESCAPED == 0,
                "an escaped name without one"
            );
            ensure!(
                ev.flags & COMLINE_EVENT_ONE_WAY == 0 || ev.request_id == 0,
                "a one-way request with id {}",
                ev.request_id
            );
            ensure!(
                ev.flags & COMLINE_EVENT_BODY_ABSENT == 0 || ev.body == none,
                "an absent body with a span"
            );
        }
        COMLINE_EVENT_REPLY => {
            let allowed = if jsonrpc {
                COMLINE_EVENT_BODY_ABSENT
            } else {
                0
            };
            ensure!(ev.flags & !allowed == 0, "a reply's flags {:#x}", ev.flags);
            ensure!(
                (ev.protocol, ev.call_id, ev.call_name, ev.capabilities) == (0, 0, none, 0),
                "a reply with request fields"
            );
            match ev.envelope {
                COMLINE_ENVELOPE_OK => ensure!(ev.error_id == 0, "an ok reply's error id"),
                COMLINE_ENVELOPE_ERR => {}
                COMLINE_ENVELOPE_FAULT => ensure!(
                    ev.error_id == 0 && ev.body == none && ev.flags == 0,
                    "a fault with a body or an error id"
                ),
                other => return Err(format!("envelope {other}")),
            }
            ensure!(
                ev.flags & COMLINE_EVENT_BODY_ABSENT == 0 || ev.body == none,
                "an absent body with a span"
            );
        }
        COMLINE_EVENT_STALE => ensure!(
            *ev == comline_event {
                kind: COMLINE_EVENT_STALE,
                frame: ev.frame,
                ..Default::default()
            },
            "a stale event with fields"
        ),
        other => return Err(format!("event kind {other}")),
    }
    Ok(())
}

/// The ABI's encode and the reference's agree.
fn same_encoded(what: Encode<'_>, got: &Encoded, want: &Encoded) -> Result<(), String> {
    if want.either != 0 {
        ensure!(
            want.either & bit(got.status) != 0,
            "status {}, the reference refuses with {}",
            got.status,
            statuses(want.either)
        );
        return Ok(());
    }
    ensure!(
        got.status == want.status,
        "status {}, the reference {}",
        got.status,
        want.status
    );
    match got.status {
        COMLINE_OK => {
            ensure!(
                got.bytes == want.bytes,
                "wrote {:02x?}, the reference {:02x?}",
                got.bytes,
                want.bytes
            );
            if let Encode::Request { .. } = what {
                ensure!(
                    got.request_id == want.request_id,
                    "request id {}, the reference {}",
                    got.request_id,
                    want.request_id
                );
            }
        }
        COMLINE_BUFFER_TOO_SMALL => ensure!(
            got.written == want.written,
            "needs {}, the reference {}",
            got.written,
            want.written
        ),
        _ => {}
    }
    Ok(())
}

/// The ABI's feed and the reference's agree.
fn same_fed(got: &Fed, want: &Fed) -> Result<(), String> {
    ensure!(
        got.status == want.status,
        "status {}, the reference {}",
        got.status,
        want.status
    );
    if got.status != COMLINE_ERR_STATE {
        ensure!(
            got.consumed == want.consumed,
            "consumed {}, the reference {}",
            got.consumed,
            want.consumed
        );
    }
    if matches!(got.status, COMLINE_OK | COMLINE_NEED_MORE) {
        ensure!(
            got.ev == want.ev,
            "event {:?}, the reference {:?}",
            got.ev,
            want.ev
        );
    }
    Ok(())
}
