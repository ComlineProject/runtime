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

/// Returns `Err` with the message when `cond` doesn't hold.
macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!($($arg)+));
        }
    };
}

mod abi;
#[macro_use]
mod reference;
mod run;
mod status;

use abi::Ffi;
use reference::Reference;
use run::Run;
use status::{bit, statuses, NEW};

use comline_runtime_ffi::*;

/// Room for any frame a sequence makes: bodies and names are at most a few
/// hundred bytes, escaped at most six times over.
pub const PLENTY: usize = 1 << 16;
/// Bytes past `cap` that must come back untouched.
pub(super) const SENTINEL: usize = 16;
pub(super) const SENTINEL_BYTE: u8 = 0xa5;

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

// ── what sequences are made of ─────────────────────────────────────────────

/// Method names: `state_model.rs`'s, and some that need escapes in JSON.
pub const METHODS: [&str; 7] = [
    "ping",
    "send",
    "chat.Chat/send",
    "a",
    "app.Chat\"quote",
    "tab\there",
    "é/\u{1}",
];

/// JSON texts, as `state_model.rs` has them, for bodies under `jsonrpc@1`.
pub const JSON_TEXTS: [&str; 6] = ["null", "1", "\"x\"", "[1,2]", "{\"a\":true}", "-3.5"];

pub const WIRE_FORMATS: [&str; 3] = ["packed@1", "json@1", "msgpack@1"];

/// A connection for [`exchange`]: a handshake, room for two waiting calls.
pub fn exchange_config(framing: &str, stream: u8) -> Config {
    Config {
        framing: framing.as_bytes().to_vec(),
        wire_format: b"json@1".to_vec(),
        ir_hash: 7,
        capabilities: 3,
        max_frame: 0,
        max_waiting: 2,
        handshake: COMLINE_HANDSHAKE_COMLINE,
        stream,
        unordered: 0,
        reserved: 0,
    }
}

/// A fixed exchange through every event kind: both handshake frames (the
/// peer's in two chunks), a call short of room then with exactly enough, a
/// request from the peer and its answer, a reply and a stale one, and a
/// malformed frame.
pub fn exchange() -> Vec<Op> {
    vec![
        Op::Start(Cap::Exact),
        Op::PeerHello {
            hello: Hello::Agree,
            capabilities: 9,
        },
        Op::Deliver(3),
        Op::Deliver(0),
        Op::Call {
            protocol: 1,
            call_id: 2,
            name: b"chat.Chat/send".to_vec(),
            expects_reply: 1,
            body: b"[1]".to_vec(),
            cap: Cap::Short(0),
        },
        Op::Call {
            protocol: 1,
            call_id: 2,
            name: b"chat.Chat/send".to_vec(),
            expects_reply: 1,
            body: b"[1]".to_vec(),
            cap: Cap::Exact,
        },
        Op::PeerRequest {
            protocol: 0,
            call_id: 4,
            name: "app.Chat\"quote".into(),
            request_id: 11,
            one_way: false,
            body: b"{}".to_vec(),
        },
        Op::Deliver(0),
        Op::Answer {
            protocol: 0,
            to: Theirs::Received(0),
            envelope: COMLINE_ENVELOPE_ERR,
            error_id: 5,
            body: b"{}".to_vec(),
            cap: Cap::Plenty,
        },
        Op::PeerReply {
            to: Ours::Waiting(0),
            protocol: 1,
            envelope: PeerEnvelope::Ok,
            body: b"true".to_vec(),
        },
        Op::PeerReply {
            to: Ours::Spent(0),
            protocol: 1,
            envelope: PeerEnvelope::Fault,
            body: Vec::new(),
        },
        Op::Deliver(0),
        Op::IsWaiting,
        Op::Abandon(Ours::Never(0)),
        Op::PeerRaw {
            framed: true,
            bytes: b"!".to_vec(),
        },
        Op::Deliver(0),
    ]
}
