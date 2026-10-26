// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One sequence of operations on a `comline_connection`, checked as it
//! runs: by the ABI's own rules alone ([`Mode::Abi`]), or also against a
//! reference built from `comline-runtime`'s public API ([`Mode::Differential`]).
//!
//! Shared by `tests/differential.rs` (proptest) and the `ffi_feed` and
//! `ffi_vs_core` fuzz targets (`runtime/fuzz/src/ffi.rs` includes this file
//! by path), so a sequence fails the same way under both. Unlike the rest of
//! `common`, it touches `comline-runtime`: the reference is the point.
//!
//! **What a sequence is.** A [`Config`] for `comline_connection_new`, then
//! [`Op`]s: this side's calls (`start`, `encode_request`, `encode_reply`,
//! `abandon`, `is_waiting`), frames the peer sends (written by the core's
//! framings, or raw bytes), byte flips in what the peer sent, and
//! deliveries, which hand queued bytes to `feed` in chunks (stream mode) or
//! one message at a time (message mode).
//!
//! **The ABI's rules, checked on every call** (both modes):
//! - no call returns `COMLINE_ERR_POISONED` (a panic crossed), and each
//!   returns only the codes its doc comment lists;
//! - no call but `comline_connection_new` allocates, when the caller passes
//!   an allocation counter;
//! - every span in an event lies inside the frame, and the frame inside the
//!   input of that `feed`; `*consumed` and the frame are what the length
//!   prefix (stream) or the message (message mode) says; a field not valid
//!   for the kind and flags is 0;
//! - `NEED_MORE` reports the frame's total size (0 before the prefix is
//!   whole), and the next `feed` given that many bytes doesn't need more;
//! - `BUFFER_TOO_SMALL` reports a size that is more than `cap`, the same
//!   size again one byte short of it, and a retry with exactly that room
//!   succeeds; it, `BUSY` and `FRAME_TOO_LARGE` spend no request id (ids
//!   count up from 0, one per frame written);
//! - `BUSY` exactly when `max_waiting` calls wait; `is_waiting` and every
//!   reply event agree with the calls this side made, answered and
//!   abandoned (in ordered mode a reply answers the innermost);
//! - requests and replies are `COMLINE_ERR_STATE` until both handshake
//!   frames have passed; the first frame fed is the peer's handshake; and
//!   after a refused handshake or an oversized stream prefix, every call is
//!   `COMLINE_ERR_STATE`.
//!
//! **The reference** ([`Mode::Differential`]) does what the ABI promises
//! from `State`, the connection framings and `HandshakeFrame` directly, the
//! way a Rust host would: sizes from encoding into a `Vec`, the stream
//! prefix read by hand. Each call must give the same status, bytes,
//! `*written`, request id, `*consumed` and event (kind, flags, ids,
//! envelope, and spans at the same offsets). Where the ABI doesn't say
//! which of two refusals wins (a bad argument on a connection that isn't
//! open, say), either is taken.

#![allow(dead_code)]

use std::collections::{BTreeSet, VecDeque};
use std::ptr;

use comline_runtime::connection::{Event, State};
use comline_runtime::contract::{
    ConnectionFraming, Datagram2Framing, Envelope, HandshakeFrame, RequestCall, RuntimeError,
    SliceBuf, FRAMING_DATAGRAM2, FRAMING_JSONRPC,
};
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime_ffi::*;

/// Returns `Err` with the message when `cond` doesn't hold.
macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!($($arg)+));
        }
    };
}

/// Room for any frame a sequence makes: bodies and names are at most a few
/// hundred bytes, escaped at most six times over.
pub const PLENTY: usize = 1 << 16;
/// Bytes past `cap` that must come back untouched.
const SENTINEL: usize = 16;
const SENTINEL_BYTE: u8 = 0xa5;

/// The settings `comline_connection_new` reads, any value allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub framing: Vec<u8>,
    pub wire_format: Vec<u8>,
    pub ir_hash: u64,
    pub capabilities: u32,
    pub max_frame: u64,
    pub max_waiting: u32,
    pub handshake: u8,
    pub stream: u8,
    pub unordered: u8,
    pub reserved: u8,
}

/// How much room an encode gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// [`PLENTY`].
    Plenty,
    /// None first, then exactly the size `BUFFER_TOO_SMALL` reported.
    Exact,
    /// None first, then 1 to `n + 1` bytes short of the reported size.
    Short(u16),
    /// This many bytes.
    Fixed(u16),
}

/// A request id of this side's: which call a reply answers or an abandon
/// gives up on. As `ReplyTo` in `core/tests/state_model.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ours {
    /// A waiting call, index 0 the innermost (newest). Falls back to
    /// `Spent` when nothing waits.
    Waiting(u16),
    /// An id handed out and no longer waited on (answered, abandoned or
    /// one-way). Falls back to the next id.
    Spent(u16),
    /// The next id plus this: never handed out.
    Never(u64),
    /// This id, whatever it is.
    Raw(u64),
}

/// A request id of the peer's, for `encode_reply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theirs {
    /// One the peer's requests carried (0 before any came).
    Received(u16),
    Raw(u64),
}

/// The peer's `comline@1` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hello {
    /// The names and schema this side has.
    Agree,
    /// Another schema.
    IrHash,
    /// Another wire format.
    WireFormat,
    /// Another framing.
    Framing,
    /// An agreeing frame cut to `n % 31` bytes.
    Short(u8),
    /// An agreeing frame with `n` bytes after it, which are ignored.
    Trailing(u8),
}

/// A reply's envelope, as the peer writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerEnvelope {
    Ok,
    Err(u16),
    Fault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `comline_connection_start`.
    Start(Cap),
    /// `comline_connection_encode_request`. `expects_reply` and `name` go in
    /// as they are, so they may be refused.
    Call {
        protocol: u16,
        call_id: u16,
        name: Vec<u8>,
        expects_reply: u8,
        body: Vec<u8>,
        cap: Cap,
    },
    /// `comline_connection_encode_reply`; `envelope` as it is.
    Answer {
        protocol: u16,
        to: Theirs,
        envelope: u8,
        error_id: u16,
        body: Vec<u8>,
        cap: Cap,
    },
    Abandon(Ours),
    IsWaiting,
    /// The peer sends its handshake frame.
    PeerHello {
        hello: Hello,
        capabilities: u32,
    },
    /// The peer sends a request, framed by the core.
    PeerRequest {
        protocol: u16,
        call_id: u16,
        name: String,
        request_id: u64,
        one_way: bool,
        body: Vec<u8>,
    },
    /// The peer sends a reply, framed by the core.
    PeerReply {
        to: Ours,
        protocol: u16,
        envelope: PeerEnvelope,
        body: Vec<u8>,
    },
    /// The peer sends these bytes: as a frame (behind a length prefix in
    /// stream mode) or, unframed, straight into the stream.
    PeerRaw {
        framed: bool,
        bytes: Vec<u8>,
    },
    /// Stream mode: move this many queued bytes (0: all of them) to the read
    /// buffer, and feed until it needs more. Message mode: feed the next
    /// message, cut to this many bytes (0: whole).
    Deliver(u16),
    /// Flip bits in a byte the peer sent and nothing has read yet, counted
    /// from the newest.
    Corrupt {
        at: u16,
        xor: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The ABI's rules alone.
    Abi,
    /// The rules, and the same calls against the reference.
    Differential,
}

/// Runs `ops` on a connection made from `config`. `allocations`, when
/// given, counts the calling thread's allocations; every call but
/// `comline_connection_new` must leave it unchanged.
pub fn run(
    config: &Config,
    ops: &[Op],
    mode: Mode,
    allocations: Option<fn() -> usize>,
) -> Result<(), String> {
    let (status, ffi) = Ffi::new(config, allocations);
    ensure!(
        NEW.contains(&status),
        "comline_connection_new: status {status}"
    );
    let reference = match mode {
        Mode::Abi => None,
        Mode::Differential => match Reference::new(config) {
            Ok(reference) => {
                ensure!(
                    status == COMLINE_OK,
                    "comline_connection_new refused a config the reference takes: {status}"
                );
                Some(reference)
            }
            Err(either) => {
                ensure!(
                    either & bit(status) != 0,
                    "comline_connection_new: status {status}, the reference refuses with {}",
                    statuses(either)
                );
                None
            }
        },
    };
    let Some(ffi) = ffi else {
        return Ok(());
    };
    let mut run = Run::new(config, ffi, reference);
    for (n, op) in ops.iter().enumerate() {
        run.step(op).map_err(|e| format!("step {n}, {op:?}: {e}"))?;
    }
    Ok(())
}

// ── status codes ───────────────────────────────────────────────────────────

/// What each function's doc comment says it returns.
const NEW: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_UNKNOWN_FRAMING,
];
const START: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_BUFFER_TOO_SMALL,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
const FEED: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_NEED_MORE,
    COMLINE_ERR_FRAMING,
    COMLINE_ERR_HANDSHAKE,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
const REQUEST: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_BUFFER_TOO_SMALL,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_BUSY,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
const REPLY: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_BUFFER_TOO_SMALL,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
const PLAIN: &[comline_status] = &[COMLINE_OK, COMLINE_ERR_INVALID_ARGUMENT, COMLINE_ERR_STATE];

/// A set of statuses, as bits.
fn bit(status: comline_status) -> u32 {
    1u32.checked_shl(status as u32).unwrap_or(0)
}

fn statuses(set: u32) -> String {
    let all: Vec<_> = (0..32).filter(|s| set & (1 << s) != 0).collect();
    format!("{all:?}")
}

// ── the ABI side ───────────────────────────────────────────────────────────

/// What one `start` or `encode_*` call gave.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Encoded {
    status: comline_status,
    written: usize,
    request_id: u64,
    /// `out[..written]` on `COMLINE_OK`.
    bytes: Vec<u8>,
    /// Reference only: when not 0, the refusals any of which is right, and
    /// nothing else is compared.
    either: u32,
}

/// What one `feed` call gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fed {
    status: comline_status,
    consumed: usize,
    ev: comline_event,
}

/// A handle, called through the `extern "C"` functions only.
struct Ffi {
    conn: *mut comline_connection,
    /// The output buffer, reused so a call allocates nothing on this side
    /// either; `SENTINEL` bytes past `cap` are checked after each call.
    out: Vec<u8>,
    allocations: Option<fn() -> usize>,
    /// The connection speaks `jsonrpc@1`.
    jsonrpc: bool,
}

impl Drop for Ffi {
    fn drop(&mut self) {
        unsafe { comline_connection_free(self.conn) };
    }
}

/// A pointer to `bytes`, null when there are none (which the ABI allows).
fn ptr_of(bytes: &[u8]) -> *const u8 {
    if bytes.is_empty() {
        ptr::null()
    } else {
        bytes.as_ptr()
    }
}

impl Ffi {
    fn new(config: &Config, allocations: Option<fn() -> usize>) -> (comline_status, Option<Self>) {
        let cfg = comline_config {
            ir_hash: config.ir_hash,
            wire_format: ptr_of(&config.wire_format),
            wire_format_len: config.wire_format.len(),
            framing: ptr_of(&config.framing),
            framing_len: config.framing.len(),
            max_frame: config.max_frame,
            capabilities: config.capabilities,
            max_waiting: config.max_waiting,
            handshake: config.handshake,
            stream: config.stream,
            unordered: config.unordered,
            _reserved: config.reserved,
        };
        let mut conn = ptr::null_mut();
        let status = unsafe { comline_connection_new(&cfg, &mut conn) };
        if status != COMLINE_OK {
            assert!(conn.is_null(), "an error leaves *out null");
            return (status, None);
        }
        assert!(!conn.is_null());
        let ffi = Self {
            conn,
            out: vec![0; PLENTY + SENTINEL],
            allocations,
            jsonrpc: config.framing == FRAMING_JSONRPC.as_bytes(),
        };
        (status, Some(ffi))
    }

    /// Runs one call, failing when it allocated.
    fn measured<R>(&self, what: &str, call: impl FnOnce() -> R) -> Result<R, String> {
        let (result, allocated) = self.counted(call);
        ensure!(allocated == 0, "{what} allocated ({allocated} allocations)");
        Ok(result)
    }

    /// Runs one call, and counts what it allocated (0 without a counter).
    fn counted<R>(&self, call: impl FnOnce() -> R) -> (R, usize) {
        let before = self.allocations.map(|count| count());
        let result = call();
        match (self.allocations, before) {
            (Some(count), Some(before)) => (result, count() - before),
            _ => (result, 0),
        }
    }

    /// One encode into `cap` bytes, through `raw(out, cap, written,
    /// request_id)`.
    fn encode_once(
        &mut self,
        what: &str,
        codes: &[comline_status],
        cap: usize,
        raw: &dyn Fn(*mut u8, usize, &mut usize, &mut u64) -> comline_status,
    ) -> Result<Encoded, String> {
        if self.out.len() < cap + SENTINEL {
            self.out.resize(cap + SENTINEL, 0);
        }
        let out = if cap == 0 {
            ptr::null_mut()
        } else {
            self.out[cap..cap + SENTINEL].fill(SENTINEL_BYTE);
            self.out.as_mut_ptr()
        };
        let (mut written, mut request_id) = (usize::MAX, u64::MAX);
        let status = self.measured(what, || raw(out, cap, &mut written, &mut request_id))?;
        ensure!(codes.contains(&status), "{what}: status {status}");
        ensure!(
            cap == 0
                || self.out[cap..cap + SENTINEL]
                    .iter()
                    .all(|&b| b == SENTINEL_BYTE),
            "{what} wrote past cap {cap}"
        );
        let bytes = match status {
            COMLINE_OK => {
                ensure!(written <= cap, "{what}: wrote {written} into {cap}");
                self.out[..written].to_vec()
            }
            COMLINE_BUFFER_TOO_SMALL => {
                ensure!(
                    written > cap,
                    "{what}: BUFFER_TOO_SMALL needing {written}, with {cap}"
                );
                Vec::new()
            }
            _ => Vec::new(),
        };
        Ok(Encoded {
            status,
            written,
            request_id,
            bytes,
            either: 0,
        })
    }

    /// An encode, and on `BUFFER_TOO_SMALL` the same one a byte short of the
    /// size it reported, which must report that size again.
    fn encode(
        &mut self,
        what: &str,
        codes: &[comline_status],
        cap: usize,
        raw: &dyn Fn(*mut u8, usize, &mut usize, &mut u64) -> comline_status,
    ) -> Result<Encoded, String> {
        let got = self.encode_once(what, codes, cap, raw)?;
        if got.status == COMLINE_BUFFER_TOO_SMALL {
            let again = self.encode_once(what, codes, got.written - 1, raw)?;
            ensure!(
                (again.status, again.written) == (COMLINE_BUFFER_TOO_SMALL, got.written),
                "{what}: BUFFER_TOO_SMALL needing {}, then {} needing {} with a byte less",
                got.written,
                again.status,
                again.written
            );
        }
        Ok(got)
    }

    fn start(&mut self, cap: usize) -> Result<Encoded, String> {
        let conn = self.conn;
        self.encode("start", START, cap, &|out, cap, written, _| unsafe {
            comline_connection_start(conn, out, cap, written)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn request(
        &mut self,
        protocol: u16,
        call_id: u16,
        name: &[u8],
        expects_reply: u8,
        body: &[u8],
        cap: usize,
    ) -> Result<Encoded, String> {
        let conn = self.conn;
        let (name_ptr, body_ptr) = (ptr_of(name), ptr_of(body));
        self.encode(
            "encode_request",
            REQUEST,
            cap,
            &|out, cap, written, request_id| unsafe {
                comline_connection_encode_request(
                    conn,
                    protocol,
                    call_id,
                    name_ptr,
                    name.len(),
                    expects_reply,
                    body_ptr,
                    body.len(),
                    out,
                    cap,
                    written,
                    request_id,
                )
            },
        )
    }

    fn reply(
        &mut self,
        protocol: u16,
        request_id: u64,
        envelope: u8,
        error_id: u16,
        body: &[u8],
        cap: usize,
    ) -> Result<Encoded, String> {
        let conn = self.conn;
        let body_ptr = ptr_of(body);
        self.encode("encode_reply", REPLY, cap, &|out, cap, written, _| unsafe {
            comline_connection_encode_reply(
                conn,
                protocol,
                request_id,
                envelope,
                error_id,
                body_ptr,
                body.len(),
                out,
                cap,
                written,
            )
        })
    }

    fn abandon(&mut self, request_id: u64) -> Result<comline_status, String> {
        let conn = self.conn;
        let status = self.measured("abandon", || unsafe {
            comline_connection_abandon(conn, request_id)
        })?;
        ensure!(PLAIN.contains(&status), "abandon: status {status}");
        Ok(status)
    }

    fn is_waiting(&mut self) -> Result<(comline_status, u8), String> {
        let conn = self.conn;
        let mut waiting = 0xff;
        let status = self.measured("is_waiting", || unsafe {
            comline_connection_is_waiting(conn, &mut waiting)
        })?;
        ensure!(PLAIN.contains(&status), "is_waiting: status {status}");
        ensure!(
            status != COMLINE_OK || waiting <= 1,
            "is_waiting: *waiting = {waiting}"
        );
        Ok((status, waiting))
    }

    fn feed(&mut self, input: &[u8]) -> Result<Fed, String> {
        let conn = self.conn;
        let mut consumed = usize::MAX;
        // Not a valid event, so a field the call leaves alone shows.
        let mut ev = comline_event {
            kind: 0xff,
            flags: 0xff,
            _reserved: 0xff,
            request_id: u64::MAX,
            ..Default::default()
        };
        let (status, allocated) = self.counted(|| unsafe {
            comline_connection_feed(conn, input.as_ptr(), input.len(), &mut consumed, &mut ev)
        });
        // The known breaches of the rule, both serde_json's under
        // `jsonrpc@1`: refusing a frame (it boxes every error) and a member
        // name with an escape (it unescapes into a scratch `Vec`). See
        // `rules.rs`, `jsonrpc_decode_allocates_nothing`, ignored until
        // they're fixed. Anything else that allocates fails.
        let frame = &input[..consumed.min(input.len())];
        let known = self.jsonrpc && (status == COMLINE_ERR_FRAMING || frame.contains(&b'\\'));
        ensure!(
            allocated == 0 || known,
            "feed allocated ({allocated} allocations)"
        );
        ensure!(FEED.contains(&status), "feed: status {status}");
        Ok(Fed {
            status,
            consumed,
            ev,
        })
    }
}

// ── the reference ──────────────────────────────────────────────────────────

/// The connection framings, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fr {
    Datagram2,
    JsonRpc,
}

/// Runs `$body` with `$f` bound to the framing as its concrete type.
macro_rules! framed {
    ($fr:expr, |$f:ident| $body:expr) => {
        match $fr {
            Fr::Datagram2 => {
                let $f = &Datagram2Framing;
                $body
            }
            Fr::JsonRpc => {
                let $f = &JsonRpcFraming;
                $body
            }
        }
    };
}

/// What the ABI promises, from the core's public API: `State` and the
/// framings for the frames, `HandshakeFrame` for the handshake, sizes from
/// encoding into a `Vec`, and the stream prefix read by hand.
struct Reference {
    state: State,
    fr: Fr,
    ours: Option<HandshakeFrame>,
    sent: bool,
    received: bool,
    closed: bool,
    stream: bool,
    max_frame: usize,
    /// The id `State` hands out next: 0, then one past the last it gave.
    next_id: u64,
}

/// Where `part` lies in `input`, when it borrows from it.
fn span_of(input: &[u8], part: &[u8]) -> Option<comline_span> {
    let base = input.as_ptr() as usize;
    let at = part.as_ptr() as usize;
    if at < base || at + part.len() > base + input.len() {
        return None;
    }
    Some(comline_span {
        offset: u32::try_from(at - base).ok()?,
        len: u32::try_from(part.len()).ok()?,
    })
}

impl Reference {
    /// The connection, or every refusal that applies to `config`.
    fn new(config: &Config) -> Result<Self, u32> {
        let mut either = 0;
        let framing = std::str::from_utf8(&config.framing).ok();
        let wire_format = std::str::from_utf8(&config.wire_format).ok();
        if framing.is_none() || wire_format.is_none() {
            either |= bit(COMLINE_ERR_INVALID_ARGUMENT);
        }
        if config.stream > 1
            || config.unordered > 1
            || config.reserved != 0
            || config.max_frame > COMLINE_MAX_FRAME_CEILING
            || !matches!(
                config.handshake,
                COMLINE_HANDSHAKE_COMLINE | COMLINE_HANDSHAKE_NONE
            )
        {
            either |= bit(COMLINE_ERR_INVALID_ARGUMENT);
        }
        let fr = match framing {
            Some(FRAMING_DATAGRAM2) => Some(Fr::Datagram2),
            Some(FRAMING_JSONRPC) => Some(Fr::JsonRpc),
            Some(_) => {
                either |= bit(COMLINE_ERR_UNKNOWN_FRAMING);
                None
            }
            None => None,
        };
        let (Some(fr), Some(framing), Some(wire_format), 0) = (fr, framing, wire_format, either)
        else {
            return Err(either);
        };
        let ours = (config.handshake == COMLINE_HANDSHAKE_COMLINE).then(|| {
            HandshakeFrame::new(config.ir_hash, wire_format, framing, config.capabilities)
        });
        let capacity = config.max_waiting as usize;
        Ok(Self {
            state: if config.unordered == 1 {
                State::unordered_with_capacity(capacity)
            } else {
                State::with_capacity(capacity)
            },
            fr,
            ours,
            sent: ours.is_none(),
            received: ours.is_none(),
            closed: false,
            stream: config.stream == 1,
            max_frame: match config.max_frame {
                0 => COMLINE_DEFAULT_MAX_FRAME as usize,
                n => n as usize,
            },
            next_id: 0,
        })
    }

    fn refused(either: u32) -> Encoded {
        Encoded {
            status: COMLINE_ERR_STATE,
            written: 0,
            request_id: 0,
            bytes: Vec::new(),
            either,
        }
    }

    fn status(status: comline_status, written: usize) -> Encoded {
        Encoded {
            status,
            written,
            request_id: 0,
            bytes: Vec::new(),
            either: 0,
        }
    }

    /// The outcome of sending `frame` with `cap` bytes of room: too large
    /// for the connection, too large for `cap`, or written behind its
    /// prefix.
    fn sized(&self, frame: &[u8], cap: usize) -> Encoded {
        if frame.len() > self.max_frame {
            return Self::status(COMLINE_ERR_FRAME_TOO_LARGE, 0);
        }
        let mut bytes = Vec::new();
        if self.stream {
            bytes.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        }
        bytes.extend_from_slice(frame);
        if bytes.len() > cap {
            return Self::status(COMLINE_BUFFER_TOO_SMALL, bytes.len());
        }
        Encoded {
            status: COMLINE_OK,
            written: bytes.len(),
            request_id: 0,
            bytes,
            either: 0,
        }
    }

    fn start(&mut self, cap: usize) -> Encoded {
        if self.closed {
            return Self::refused(bit(COMLINE_ERR_STATE));
        }
        let Some(ours) = self.ours else {
            return Self::status(COMLINE_OK, 0);
        };
        if self.sent {
            return Self::refused(bit(COMLINE_ERR_STATE));
        }
        let mut frame = Vec::new();
        ours.encode(&mut frame);
        let got = self.sized(&frame, cap);
        self.sent |= got.status == COMLINE_OK;
        got
    }

    /// The refusals that apply before anything is framed.
    fn before(&self, bad_argument: bool) -> u32 {
        let mut either = 0;
        if self.closed || !(self.sent && self.received) {
            either |= bit(COMLINE_ERR_STATE);
        }
        if bad_argument && !self.closed {
            either |= bit(COMLINE_ERR_INVALID_ARGUMENT);
        }
        either
    }

    #[allow(clippy::too_many_arguments)]
    fn request(
        &mut self,
        protocol: u16,
        call_id: u16,
        name: &[u8],
        expects_reply: u8,
        body: &[u8],
        cap: usize,
    ) -> Result<Encoded, String> {
        let name = std::str::from_utf8(name).ok();
        let either = self.before(name.is_none() || expects_reply > 1);
        let (Some(name), 0) = (name, either) else {
            return Ok(Self::refused(either));
        };
        let expects = expects_reply == 1;
        // No room at all: `Busy` if the call can't wait, and otherwise a
        // frame that doesn't fit, which commits nothing.
        let probe = framed!(self.fr, |f| self.state.encode_request_body(
            f,
            protocol,
            call_id,
            name,
            body,
            expects,
            &mut SliceBuf::new(&mut [])
        ));
        match probe {
            Err(RuntimeError::Busy) => return Ok(Self::status(COMLINE_ERR_BUSY, 0)),
            Err(RuntimeError::BufferTooSmall) => {}
            other => {
                return Err(format!(
                    "reference: an empty buffer took a frame: {other:?}"
                ))
            }
        }
        let mut frame = Vec::new();
        framed!(self.fr, |f| f.encode_request_body(
            protocol,
            call_id,
            name,
            self.next_id,
            !expects,
            body,
            &mut frame
        ));
        let mut got = self.sized(&frame, cap);
        if got.status != COMLINE_OK {
            return Ok(got);
        }
        let mut committed = Vec::new();
        let id = framed!(self.fr, |f| self.state.encode_request_body(
            f,
            protocol,
            call_id,
            name,
            body,
            expects,
            &mut committed
        ))
        .map_err(|e| format!("reference: {e:?}"))?;
        ensure!(
            id == self.next_id && committed == frame,
            "reference: State gave id {id} and {committed:02x?}, after {frame:02x?} for {}",
            self.next_id
        );
        self.next_id = id.wrapping_add(1);
        got.request_id = id;
        Ok(got)
    }

    fn reply(
        &mut self,
        protocol: u16,
        request_id: u64,
        envelope: u8,
        error_id: u16,
        body: &[u8],
        cap: usize,
    ) -> Encoded {
        let envelope = match envelope {
            COMLINE_ENVELOPE_OK => Some(Envelope::Ok(body)),
            COMLINE_ENVELOPE_ERR => Some(Envelope::Err { id: error_id, body }),
            COMLINE_ENVELOPE_FAULT if body.is_empty() => Some(Envelope::Fault),
            _ => None,
        };
        let either = self.before(envelope.is_none());
        let (Some(envelope), 0) = (envelope, either) else {
            return Self::refused(either);
        };
        let mut frame = Vec::new();
        framed!(self.fr, |f| self
            .state
            .encode_reply(f, protocol, request_id, &envelope, &mut frame));
        self.sized(&frame, cap)
    }

    fn abandon(&mut self, request_id: u64) -> comline_status {
        if self.closed {
            return COMLINE_ERR_STATE;
        }
        self.state.abandon(request_id);
        COMLINE_OK
    }

    fn is_waiting(&self) -> (comline_status, u8) {
        if self.closed {
            return (COMLINE_ERR_STATE, 0);
        }
        (COMLINE_OK, u8::from(self.state.is_waiting()))
    }

    fn feed(&mut self, input: &[u8]) -> Result<Fed, String> {
        let mut fed = Fed {
            status: COMLINE_OK,
            consumed: 0,
            ev: comline_event::default(),
        };
        if self.closed {
            fed.status = COMLINE_ERR_STATE;
            return Ok(fed);
        }
        let (start, len) = if self.stream {
            let Some(prefix) = input.first_chunk::<4>() else {
                fed.status = COMLINE_NEED_MORE;
                return Ok(fed);
            };
            let len = u32::from_le_bytes(*prefix) as usize;
            if len > self.max_frame {
                self.closed = true;
                fed.status = COMLINE_ERR_FRAME_TOO_LARGE;
                return Ok(fed);
            }
            if 4 + len > input.len() {
                fed.status = COMLINE_NEED_MORE;
                fed.ev.frame.len = (4 + len) as u32;
                return Ok(fed);
            }
            (4, len)
        } else {
            if input.len() > self.max_frame {
                fed.status = COMLINE_ERR_FRAME_TOO_LARGE;
                return Ok(fed);
            }
            (0, input.len())
        };
        let frame = &input[start..start + len];
        fed.consumed = start + len;
        fed.ev.frame = span_of(input, frame).expect("the frame is in the input");

        if !self.received {
            let ours = self.ours.expect("only a handshake leaves `received` unset");
            match ours.check_bytes(frame) {
                Ok(peer) => {
                    self.received = true;
                    fed.ev.kind = COMLINE_EVENT_HANDSHAKE_OK;
                    fed.ev.capabilities = peer.capabilities;
                }
                Err(RuntimeError::Handshake) => {
                    self.closed = true;
                    fed.status = COMLINE_ERR_HANDSHAKE;
                }
                Err(other) => return Err(format!("reference: the handshake raised {other:?}")),
            }
            return Ok(fed);
        }

        let event = match framed!(self.fr, |f| self.state.receive(f, frame)) {
            Ok(event) => event,
            Err(RuntimeError::Framing) => {
                fed.status = COMLINE_ERR_FRAMING;
                return Ok(fed);
            }
            Err(other) => return Err(format!("reference: receive raised {other:?}")),
        };
        let ev = &mut fed.ev;
        // A body that doesn't borrow the frame is JSON-RPC's absent one.
        let body = |ev: &mut comline_event, part: &[u8]| match span_of(input, part) {
            Some(span) => ev.body = span,
            None => ev.flags |= COMLINE_EVENT_BODY_ABSENT,
        };
        match event {
            Event::Request {
                protocol,
                request,
                one_way,
            } => {
                ev.kind = COMLINE_EVENT_REQUEST;
                if let Some(protocol) = protocol {
                    ev.flags |= COMLINE_EVENT_HAS_PROTOCOL;
                    ev.protocol = protocol;
                }
                if one_way {
                    ev.flags |= COMLINE_EVENT_ONE_WAY;
                }
                match request.call {
                    RequestCall::Id(id) => ev.call_id = id,
                    RequestCall::Name(name) => {
                        let (written, escaped) = name.as_written();
                        ev.flags |= COMLINE_EVENT_CALL_BY_NAME;
                        if escaped {
                            ev.flags |= COMLINE_EVENT_NAME_ESCAPED;
                        }
                        ev.call_name = span_of(input, written.as_bytes())
                            .ok_or("reference: a method name not in the frame")?;
                    }
                }
                ev.request_id = request.request_id;
                body(ev, request.params);
            }
            Event::Reply {
                request_id,
                envelope,
            } => {
                ev.kind = COMLINE_EVENT_REPLY;
                ev.request_id = request_id;
                match envelope {
                    Envelope::Ok(payload) => {
                        ev.envelope = COMLINE_ENVELOPE_OK;
                        body(ev, payload);
                    }
                    Envelope::Err { id, body: fields } => {
                        ev.envelope = COMLINE_ENVELOPE_ERR;
                        ev.error_id = id;
                        body(ev, fields);
                    }
                    Envelope::Fault => ev.envelope = COMLINE_ENVELOPE_FAULT,
                }
            }
            Event::Stale => ev.kind = COMLINE_EVENT_STALE,
        }
        Ok(fed)
    }
}

// ── the run ────────────────────────────────────────────────────────────────

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

struct Run {
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
    fn new(config: &Config, ffi: Ffi, reference: Option<Reference>) -> Self {
        let framing = String::from_utf8_lossy(&config.framing).into_owned();
        let handshake = config.handshake == COMLINE_HANDSHAKE_COMLINE;
        Self {
            ffi,
            reference,
            fr: if framing == FRAMING_JSONRPC {
                Fr::JsonRpc
            } else {
                Fr::Datagram2
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

    fn step(&mut self, op: &Op) -> Result<(), String> {
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
                COMLINE_OK | COMLINE_ERR_FRAMING | COMLINE_ERR_HANDSHAKE => fed.consumed,
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
            COMLINE_ERR_FRAMING => ensure!(self.received, "a framing error in the handshake"),
            _ => return Err("a whole frame within max_frame, refused".into()),
        }
        Ok(())
    }
}

/// Whether `inner` lies inside `outer`.
fn inside(inner: comline_span, outer: comline_span) -> bool {
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
                    "a datagram@2 request's flags {:#x}",
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
