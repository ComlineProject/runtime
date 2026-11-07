// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline-runtime-ffi`: the C ABI over `comline-runtime`.
//!
//! It carries what is bytes and integers: the `comline@1` and `comline@2`
//! handshakes, the envelopes, the connection framings (`datagram@2`,
//! `datagram@3`, `jsonrpc@1`), the connection state (request ids, waiting calls, stale replies) and the frame
//! limits. Codecs, transports, async and dispatch stay in each host. See
//! `ComlineProject/docs` → Proposals → *Shared runtime core* → *One protocol
//! core*.
//!
//! `include/comline.h` is generated from this file by cbindgen and checked
//! in. Its comments are these doc comments.
//!
//! # Rules every function keeps
//!
//! - **One crossing per frame.** `feed`, `encode_request` and `encode_reply`
//!   each handle one whole frame, and nothing calls back into the host.
//! - **Buffers are the caller's.** Every output goes to an `out` / `cap` /
//!   `*written` triple; every input is borrowed for the call only. Event
//!   spans are offsets into the caller's input, never pointers.
//! - **No allocation after `comline_connection_new`.** The handle is the one
//!   thing the library allocates, sized by `max_waiting`, and
//!   `comline_connection_free` frees it. One known exception, a bug:
//!   `comline_connection_feed` on a `jsonrpc@1` connection can allocate
//!   (runtime#58). And one by design: a `comline@2` connection allocates
//!   during its handshake, which holds lists whose size the peer picks; once
//!   the handshake is over, it allocates no more than any other connection.
//! - **Null pointers.** A data pointer (`in`, `body`, `name`, `out`, …) may
//!   be null only when its length or capacity is 0. Any other null pointer
//!   is `COMLINE_ERR_INVALID_ARGUMENT`, and so is a length or capacity above
//!   `PTRDIFF_MAX` (no object is that large).
//! - **Inputs and outputs must not overlap.** No buffer a call writes (`out`,
//!   `*written`, `*ev`, the handle, …) may share a byte with any buffer or
//!   out-pointer the same call takes; a call given overlapping ones returns
//!   `COMLINE_ERR_INVALID_ARGUMENT` and touches nothing. Inputs may overlap
//!   each other.
//! - **Panics never cross.** Every function runs inside `catch_unwind`. A
//!   panic is a bug: the call returns `COMLINE_ERR_POISONED`, the handle is
//!   marked poisoned and every later call on it returns the same, so the
//!   binding drops the connection. A free function returns
//!   `COMLINE_ERR_POISONED` for the call that panicked.
//!
//!   **On wasm32 the instance is poisoned, not just the handle.** There a
//!   panic doesn't unwind: it aborts, which is a trap (a
//!   `WebAssembly.RuntimeError` in JS) that `catch_unwind` can't catch, so
//!   the module never returns `COMLINE_ERR_POISONED` itself. The trap stops
//!   the module mid-call and leaves the whole instance (its stack pointer,
//!   its heap, every handle in it) unusable. The host marks the instance
//!   dead and answers every later call on it, and on every handle from it,
//!   with `COMLINE_ERR_POISONED`, never entering the module again; then it
//!   drops the instance and makes a new one. `js/comline.mjs` does this.
//! - **Threads.** A handle may move between threads, but calls on one handle
//!   must not overlap. Different handles are independent, and the free
//!   functions are pure.
//!
//! # Versioning
//!
//! The ABI has its own version, `COMLINE_ABI_MAJOR.COMLINE_ABI_MINOR`. A
//! binding checks `comline_abi_version()` at load, before any other call;
//! before 1.0, a minor counts as a major. Status codes, event kinds and flag
//! bits are append-only: a binding maps a number it doesn't know to a
//! generic error carrying it.

#![allow(non_camel_case_types)]

use core::ptr;
use core::slice;
use std::panic::{self, AssertUnwindSafe};

use comline_runtime::comline2::{
    Comline2, Exchange, Message, Negotiated, NewerConsumer, Params, ProtocolState, Refusal,
    SameVersionDiffers, Scope, Step,
};
use comline_runtime::connection::{Event, State};
use comline_runtime::contract::stream::{self, Split, DEFAULT_MAX_FRAME, PREFIX_LEN};
use comline_runtime::contract::{
    name_hash, BufMut, ConnectionFraming, CountBuf, Datagram2Framing, Datagram3Framing, Envelope,
    HandshakeFrame, MethodName, Reason, RequestCall, RuntimeError, Side, SliceBuf,
    FRAMING_DATAGRAM2, FRAMING_DATAGRAM3, FRAMING_JSONRPC, HANDSHAKE_COMLINE, HANDSHAKE_COMLINE2,
    HANDSHAKE_NONE,
};
use comline_runtime::framing::JsonRpcFraming;

// ── versions ───────────────────────────────────────────────────────────────

/// The ABI's major version: it changes when an existing signature, struct
/// layout or code meaning changes. 0 until the ABI is frozen at 1.0.
pub const COMLINE_ABI_MAJOR: u32 = 0;
/// The ABI's minor version: it changes when functions, codes or flag bits
/// are added. Before 1.0 a binding accepts only its own minor.
pub const COMLINE_ABI_MINOR: u32 = 2;

/// The library's ABI version, `(major << 16) | minor`.
#[no_mangle]
pub extern "C" fn comline_abi_version() -> u32 {
    (COMLINE_ABI_MAJOR << 16) | COMLINE_ABI_MINOR
}

// ── status codes ───────────────────────────────────────────────────────────

/// What a call returns: `COMLINE_OK`, an outcome that isn't an error (below
/// 16), or an error (16 and up). Append-only, stable values.
pub type comline_status = i32;

/// Done.
pub const COMLINE_OK: comline_status = 0;
/// Stream mode: the input holds no whole frame yet. Nothing was consumed;
/// `ev->frame.len` is the total number of bytes the frame takes, prefix
/// included, once the prefix is complete (0 before that).
pub const COMLINE_NEED_MORE: comline_status = 1;
/// The output didn't fit `cap`. `*written` holds the exact size needed, and
/// nothing changed: no request id was spent and nothing waits.
pub const COMLINE_BUFFER_TOO_SMALL: comline_status = 2;
/// A malformed frame, or (ordered connections) a reply that skips the
/// innermost waiting call. The frame was consumed and the connection stays
/// usable; whether to drop it is the host's choice.
pub const COMLINE_ERR_FRAMING: comline_status = 16;
/// The peer's `comline@1` handshake frame didn't parse, or names another
/// schema, wire format or framing. The connection is closed.
pub const COMLINE_ERR_HANDSHAKE: comline_status = 17;
/// A frame over the connection's `max_frame`.
///
/// - Encoding: nothing was written, no request id was spent, and the
///   connection keeps working.
/// - Feeding in stream mode: a length prefix over the limit. The stream
///   can't be resynchronised past it, so the connection is closed and later
///   calls return `COMLINE_ERR_STATE`.
/// - Feeding in message mode: the frame is refused and the connection keeps
///   working.
pub const COMLINE_ERR_FRAME_TOO_LARGE: comline_status = 18;
/// A framing name this library has no connection form of. See
/// `comline_supports`.
pub const COMLINE_ERR_UNKNOWN_FRAMING: comline_status = 19;
/// A null pointer where one isn't allowed, a name that isn't UTF-8, or a
/// field outside its values.
pub const COMLINE_ERR_INVALID_ARGUMENT: comline_status = 20;
/// The call doesn't fit the connection's state: a request or reply before
/// the handshake completed, a second `comline_connection_start`, or any call
/// on a closed connection.
pub const COMLINE_ERR_STATE: comline_status = 21;
/// An earlier call on this handle panicked (a bug); drop the handle. A free
/// function returns it for the call that panicked.
pub const COMLINE_ERR_POISONED: comline_status = 22;
/// The request wasn't made: the connection already waits on `max_waiting`
/// calls. Nothing was written and no request id was spent; queueing is the
/// binding's choice.
pub const COMLINE_ERR_BUSY: comline_status = 23;
/// Fed a `jsonrpc@1` error with a `null` id: the peer says it couldn't read a
/// request of this side's, and not which. Like `COMLINE_ERR_FRAMING` it changes
/// nothing on the connection, and every call still waits, so the host decides
/// whether to go on; a peer that can't read one frame usually can't read any.
pub const COMLINE_ERR_PEER_REFUSED: comline_status = 24;
/// The `comline@2` handshake refused the connection, this side or the peer:
/// `comline_connection_refusal` says why and which. It can leave a last
/// message to send (`COMLINE_EVENT_SEND`); after that the connection is closed.
pub const COMLINE_ERR_REFUSED: comline_status = 25;
/// `comline_connection_check_call`: the peer can't take this call, because it
/// lacks the function or the protocol was refused. Fail the call locally;
/// nothing has reached the wire.
pub const COMLINE_ERR_UNAVAILABLE: comline_status = 26;

/// The status for an error the core returned. Only the sans-IO pieces run
/// here (framings, `State`, `contract::stream`, the handshake frame), and
/// they raise only these. `Transport`, `Serialization`, `Timeout`,
/// `UnknownCall`, `Remote` and `Provider` are raised by a host's transport,
/// codec, clock or dispatch, never across this ABI, so one reaching here is
/// a bug, handled as a panic is: the handle is poisoned.
fn status_of(error: RuntimeError) -> comline_status {
    match error {
        RuntimeError::Framing => COMLINE_ERR_FRAMING,
        RuntimeError::Handshake => COMLINE_ERR_HANDSHAKE,
        RuntimeError::FrameTooLarge => COMLINE_ERR_FRAME_TOO_LARGE,
        RuntimeError::Busy => COMLINE_ERR_BUSY,
        RuntimeError::BufferTooSmall => COMLINE_BUFFER_TOO_SMALL,
        RuntimeError::PeerRefused => COMLINE_ERR_PEER_REFUSED,
        RuntimeError::Refused { .. } => COMLINE_ERR_REFUSED,
        RuntimeError::Unavailable => COMLINE_ERR_UNAVAILABLE,
        RuntimeError::Transport
        | RuntimeError::Serialization
        | RuntimeError::Timeout
        | RuntimeError::UnknownCall
        | RuntimeError::Remote { .. }
        | RuntimeError::Provider => {
            panic!("the sans-IO core raised a host-side error: {error:?}")
        }
        other => panic!("a RuntimeError this ABI has no code for: {other:?}"),
    }
}

// ── limits ─────────────────────────────────────────────────────────────────

/// The default frame limit, in bytes (16 MiB), not counting a stream's
/// length prefix: what `max_frame = 0` means. Every Comline runtime uses it
/// unless configured otherwise.
pub const COMLINE_DEFAULT_MAX_FRAME: u64 = 16 * 1024 * 1024;
/// The largest `max_frame` a connection takes. A stream's length prefix is a
/// `u32`, and every span (offset plus length, prefix included) has to fit a
/// `u32` too, so it is `u32::MAX` less the prefix.
pub const COMLINE_MAX_FRAME_CEILING: u64 = 4_294_967_291;
/// The stream length prefix's size in bytes: a `u32`, little-endian.
pub const COMLINE_PREFIX_LEN: u32 = 4;
/// The `comline@1` handshake frame's size in bytes.
pub const COMLINE_HANDSHAKE_LEN: u32 = 31;

// ── structs ────────────────────────────────────────────────────────────────

/// A part of the caller's input: `offset` bytes from its start, `len` bytes
/// long.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct comline_span {
    pub offset: u32,
    pub len: u32,
}

/// `comline_config.handshake`: Comline's own `comline@1`. Each end sends a
/// 31-byte frame naming its schema hash, wire format and framing, and
/// refuses a peer that names others. The default (0).
pub const COMLINE_HANDSHAKE_COMLINE: u8 = 0;
/// `comline_config.handshake`: `none`. Nothing is sent or checked, and the
/// connection is open from the start. Gives up schema and encoding
/// agreement (`@unverified` in the schema).
pub const COMLINE_HANDSHAKE_NONE: u8 = 1;
/// `comline_config.handshake`: `comline@2`, which checks compatibility by each
/// protocol's line. It needs its facts and parameters, so only
/// `comline_connection_new_comline2` takes it; `comline_connection_new` refuses
/// it with `COMLINE_ERR_INVALID_ARGUMENT`.
pub const COMLINE_HANDSHAKE_COMLINE2: u8 = 2;

/// `comline_comline2_config.side` and `comline_refusal.side`: the end that
/// opened the connection (the consumer).
pub const COMLINE_SIDE_INITIATOR: u8 = 0;
/// `comline_comline2_config.side` and `comline_refusal.side`: the end that
/// accepted it (the provider).
pub const COMLINE_SIDE_RESPONDER: u8 = 1;

/// `comline_comline2_config.scope`: a protocol's refusal refuses that protocol
/// alone. The default (0).
pub const COMLINE_SCOPE_PROTOCOL: u8 = 0;
/// `comline_comline2_config.scope`: a protocol's refusal refuses the whole
/// connection (`unknown_protocol` excepted).
pub const COMLINE_SCOPE_PACKAGE: u8 = 1;
/// `comline_comline2_config.newer_consumer`: connect, and calls to the
/// functions the peer lacks fail locally. The default (0).
pub const COMLINE_NEWER_CONSUMER_DEGRADE: u8 = 0;
/// `comline_comline2_config.newer_consumer`: refuse the protocol.
pub const COMLINE_NEWER_CONSUMER_REFUSE: u8 = 1;
/// `comline_comline2_config.same_version_differs`: refuse the protocol. The
/// default (0).
pub const COMLINE_SAME_VERSION_DIFFERS_REFUSE: u8 = 0;
/// `comline_comline2_config.same_version_differs`: connect, and report the
/// protocol in `comline_connection_warnings`.
pub const COMLINE_SAME_VERSION_DIFFERS_WARN: u8 = 1;
/// `comline_comline2_config.exchange`: the initiator offers, the responder
/// answers, the initiator gives its verdict. The default (0).
pub const COMLINE_EXCHANGE_OFFER: u8 = 0;
/// `comline_comline2_config.exchange`: both send a hello, both give a verdict.
pub const COMLINE_EXCHANGE_SYMMETRIC: u8 = 1;

/// `comline_comline2_message_check`: a hello.
pub const COMLINE_COMLINE2_HELLO: u8 = 1;
/// `comline_comline2_message_check`: an answer.
pub const COMLINE_COMLINE2_ANSWER: u8 = 2;
/// `comline_comline2_message_check`: a verdict.
pub const COMLINE_COMLINE2_VERDICT: u8 = 3;

/// `comline_refusal.reason` and `comline_protocol_outcome.reason`: the peer's
/// message didn't parse. Stable, as the wire carries them; a new reason takes
/// the next free code.
pub const COMLINE_REASON_MALFORMED: u8 = 1;
/// The peer speaks another version of the handshake (a `comline@1` frame, say).
pub const COMLINE_REASON_HANDSHAKE_VERSION: u8 = 2;
/// The two ends declare different `exchange` parameters.
pub const COMLINE_REASON_EXCHANGE_MISMATCH: u8 = 3;
/// The two ends' package ids differ.
pub const COMLINE_REASON_PACKAGE_MISMATCH: u8 = 4;
/// The two ends' encodings differ, by parameter hash.
pub const COMLINE_REASON_ENCODING_MISMATCH: u8 = 5;
/// The protocol broke between the two ends' versions: its lines differ.
pub const COMLINE_REASON_LINE_MISMATCH: u8 = 6;
/// The versions are equal but the shape differs, under
/// `same_version_differs = refuse`.
pub const COMLINE_REASON_SAME_VERSION_DIFFERS: u8 = 7;
/// The consumer has functions the provider lacks, under
/// `newer_consumer = refuse`.
pub const COMLINE_REASON_NEWER_CONSUMER: u8 = 8;
/// Only one end has the protocol.
pub const COMLINE_REASON_UNKNOWN_PROTOCOL: u8 = 9;

/// How to set up a connection, read once by `comline_connection_new` or
/// `comline_connection_new_comline2`. The
/// name pointers are borrowed for that call only.
///
/// A connection always speaks a **connection framing**, whose frames say
/// whether they're a request or a reply, so either end can call the other:
/// `datagram@2`, `datagram@3` or `jsonrpc@1`. The lockstep `datagram@1` has no connection
/// form and is `COMLINE_ERR_UNKNOWN_FRAMING`. On `jsonrpc@1` a one-way call
/// is therefore a JSON-RPC notification, with no `id`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct comline_config {
    /// The schema fingerprint the generated code was built from. Only the
    /// handshake reads it.
    pub ir_hash: u64,
    /// The wire format's name (`"packed@1"`), UTF-8, not NUL-terminated. The
    /// codecs are the host's, so any name is taken; only the handshake reads
    /// it, hashed. Bodies aren't checked: on `jsonrpc@1` each must be one JSON
    /// value, and a request's params an array, an object or empty, or a peer
    /// refuses the frame.
    pub wire_format: *const u8,
    pub wire_format_len: usize,
    /// The framing's name: `"datagram@2"`, `"datagram@3"` or `"jsonrpc@1"`.
    /// UTF-8, not NUL-terminated.
    pub framing: *const u8,
    pub framing_len: usize,
    /// The largest frame sent or taken, prefix not counted. 0 is
    /// `COMLINE_DEFAULT_MAX_FRAME`. It may be lowered, or raised up to
    /// `COMLINE_MAX_FRAME_CEILING`; above that is
    /// `COMLINE_ERR_INVALID_ARGUMENT`.
    pub max_frame: u64,
    /// Transport capability bits, sent in the handshake. Advisory: peers
    /// that differ here still agree.
    pub capabilities: u32,
    /// How many calls may wait for a reply at once. The room is reserved
    /// when the handle is made; a request past it is `COMLINE_ERR_BUSY`. 0 is
    /// none: such a connection serves requests and sends one-way calls only.
    pub max_waiting: u32,
    /// `COMLINE_HANDSHAKE_COMLINE`, `COMLINE_HANDSHAKE_NONE`, or
    /// `COMLINE_HANDSHAKE_COMLINE2` with `comline_connection_new_comline2`.
    pub handshake: u8,
    /// 1: a byte stream, each frame behind a `u32` LE length prefix (TCP, a
    /// pipe, stdio). 0: one frame per message, its boundaries kept by the
    /// transport (WebSocket, UDP).
    pub stream: u8,
    /// 1: replies may come in any order (a host with several calls in
    /// flight). 0: calls nest, and a reply must answer the innermost waiting
    /// call.
    pub unordered: u8,
    /// Must be 0.
    pub _reserved: u8,
}

/// `comline_event.kind`: the peer's handshake frame arrived and agrees with
/// ours. `capabilities` holds the peer's capability bits.
pub const COMLINE_EVENT_HANDSHAKE_OK: u8 = 0;
/// `comline_event.kind`: the peer calls this side.
pub const COMLINE_EVENT_REQUEST: u8 = 1;
/// `comline_event.kind`: the reply to a call this side waits on, which no
/// longer waits.
pub const COMLINE_EVENT_REPLY: u8 = 2;
/// `comline_event.kind`: a reply to a call nobody waits on any more (it was
/// abandoned). Drop it.
pub const COMLINE_EVENT_STALE: u8 = 3;
/// `comline_event.kind`: a `comline@2` handshake message was taken in, and the
/// handshake goes on. Nothing in the event but `frame` and `COMLINE_EVENT_SEND`.
pub const COMLINE_EVENT_HANDSHAKE_CONTINUE: u8 = 4;

/// `comline_event.flags`: `protocol` holds the protocol the request is for.
/// Set on `datagram@2` and `datagram@3` requests. A `jsonrpc@1` request names only its
/// method; the protocol is whichever one this side provides with a call by
/// that name.
pub const COMLINE_EVENT_HAS_PROTOCOL: u8 = 1 << 0;
/// `comline_event.flags`: the request is one-way and wants no reply. Set
/// whenever the frame says so (a `jsonrpc@1` notification), and never
/// inferred from a request id of 0. `datagram@2` and `datagram@3` have no way
/// to say it on the wire, so there a one-way call is told apart by the schema alone.
pub const COMLINE_EVENT_ONE_WAY: u8 = 1 << 1;
/// `comline_event.flags`: the request names its call by `call_name`
/// (`jsonrpc@1`), not by `call_id`.
pub const COMLINE_EVENT_CALL_BY_NAME: u8 = 1 << 2;
/// `comline_event.flags`: the frame has no body: JSON-RPC `params` or
/// `error.data` absent, or `params`, `result` or `error.data` `null`. `body` is
/// empty, and a JSON codec reads it as `null`.
pub const COMLINE_EVENT_BODY_ABSENT: u8 = 1 << 3;
/// `comline_event.flags`: `call_name` is JSON string text with escapes in
/// it, as the frame spelled it (`app.Chat\/count`). Read it with
/// `comline_method_name_unescape`, or compare it as JSON text.
pub const COMLINE_EVENT_NAME_ESCAPED: u8 = 1 << 4;
/// `comline_event.flags`, and beside `COMLINE_ERR_REFUSED`: `comline@2` has a
/// message for the peer. Call `comline_connection_start` to get it.
pub const COMLINE_EVENT_SEND: u8 = 1 << 5;

/// `comline_event.envelope` and `comline_connection_encode_reply`: the call
/// succeeded; the body is its result.
pub const COMLINE_ENVELOPE_OK: u8 = 0;
/// The call raised a schema error: `error_id` is its schema-global ordinal
/// and the body its fields.
pub const COMLINE_ENVELOPE_ERR: u8 = 1;
/// The provider failed outside the schema. No body and no ordinal.
pub const COMLINE_ENVELOPE_FAULT: u8 = 2;

/// What one fed frame means. Spans are offsets into the input passed to
/// `comline_connection_feed`. A field not valid for the kind and flags is 0.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct comline_event {
    /// `COMLINE_EVENT_*`.
    pub kind: u8,
    /// `COMLINE_EVENT_HAS_PROTOCOL` | `ONE_WAY` | `CALL_BY_NAME` |
    /// `BODY_ABSENT` | `NAME_ESCAPED` | `SEND`.
    pub flags: u8,
    /// The protocol a request is for, with `COMLINE_EVENT_HAS_PROTOCOL`.
    pub protocol: u16,
    /// The call a request makes, without `COMLINE_EVENT_CALL_BY_NAME`.
    pub call_id: u16,
    /// For a reply: `COMLINE_ENVELOPE_*`.
    pub envelope: u8,
    pub _reserved: u8,
    /// For a reply with `COMLINE_ENVELOPE_ERR`: the schema-global ordinal.
    pub error_id: u16,
    pub _reserved2: u16,
    /// The method's name, with `COMLINE_EVENT_CALL_BY_NAME`.
    pub call_name: comline_span,
    /// A request's params, or a reply's result or error body.
    pub body: comline_span,
    /// The whole frame, prefix not included, for logging or forwarding.
    pub frame: comline_span,
    /// For `COMLINE_EVENT_HANDSHAKE_OK`: the peer's capability bits.
    pub capabilities: u32,
    /// A request's id (0 for a one-way `jsonrpc@1` request), to answer it
    /// with; or the id of the call a reply answers.
    pub request_id: u64,
}

/// A decoded `comline@1` handshake frame. The names are carried hashed
/// (`comline_name_hash`).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct comline_handshake {
    pub ir_hash: u64,
    pub wire_format: u64,
    pub framing: u64,
    pub capabilities: u32,
    pub _reserved: u32,
}

/// A `comline@2` connection's facts and parameters, read once by
/// `comline_connection_new_comline2`. `hello` is borrowed for that call only.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct comline_comline2_config {
    /// This end's facts, as an encoded hello message (`docs` → Reference →
    /// Wire → Handshakes). A generator can write them out as a constant: they
    /// don't change once the code is built. They're put in canonical order, and
    /// the `exchange` byte is replaced by `exchange` below.
    pub hello: *const u8,
    pub hello_len: usize,
    /// `COMLINE_SIDE_*`.
    pub side: u8,
    /// `COMLINE_SCOPE_*`.
    pub scope: u8,
    /// `COMLINE_NEWER_CONSUMER_*`.
    pub newer_consumer: u8,
    /// `COMLINE_SAME_VERSION_DIFFERS_*`.
    pub same_version_differs: u8,
    /// `COMLINE_EXCHANGE_*`.
    pub exchange: u8,
    /// Must be 0.
    pub _reserved: [u8; 3],
}

/// Why `comline@2` refused a connection.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct comline_refusal {
    /// `COMLINE_REASON_*`.
    pub reason: u8,
    /// Which end refused: `COMLINE_SIDE_*`.
    pub side: u8,
    /// 1 when the refusal is about one protocol, held in `protocol`.
    pub has_protocol: u8,
    pub _reserved: u8,
    pub protocol: u16,
    pub _reserved2: u16,
}

/// What one protocol of this end's came to under `comline@2`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct comline_protocol_outcome {
    /// The protocol's wire number.
    pub number: u16,
    /// 0 when it's usable, otherwise why it was refused: `COMLINE_REASON_*`.
    pub reason: u8,
    pub _reserved: u8,
}

/// The release part of a package version.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct comline_version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

// The layouts are frozen within a major; these hold them to it. `tests/c`
// checks the same numbers from C.
const _: () = {
    use core::mem::{align_of, offset_of, size_of};

    assert!(size_of::<comline_span>() == 8);
    assert!(offset_of!(comline_span, offset) == 0);
    assert!(offset_of!(comline_span, len) == 4);

    assert!(size_of::<comline_event>() == 48);
    assert!(align_of::<comline_event>() == 8);
    assert!(offset_of!(comline_event, kind) == 0);
    assert!(offset_of!(comline_event, flags) == 1);
    assert!(offset_of!(comline_event, protocol) == 2);
    assert!(offset_of!(comline_event, call_id) == 4);
    assert!(offset_of!(comline_event, envelope) == 6);
    assert!(offset_of!(comline_event, _reserved) == 7);
    assert!(offset_of!(comline_event, error_id) == 8);
    assert!(offset_of!(comline_event, _reserved2) == 10);
    assert!(offset_of!(comline_event, call_name) == 12);
    assert!(offset_of!(comline_event, body) == 20);
    assert!(offset_of!(comline_event, frame) == 28);
    assert!(offset_of!(comline_event, capabilities) == 36);
    assert!(offset_of!(comline_event, request_id) == 40);

    assert!(size_of::<comline_handshake>() == 32);
    assert!(offset_of!(comline_handshake, ir_hash) == 0);
    assert!(offset_of!(comline_handshake, wire_format) == 8);
    assert!(offset_of!(comline_handshake, framing) == 16);
    assert!(offset_of!(comline_handshake, capabilities) == 24);

    // `comline_config` holds pointers, so its layout follows the pointer
    // width.
    let p = size_of::<usize>();
    assert!(offset_of!(comline_config, ir_hash) == 0);
    assert!(offset_of!(comline_config, wire_format) == 8);
    assert!(offset_of!(comline_config, wire_format_len) == 8 + p);
    assert!(offset_of!(comline_config, framing) == 8 + 2 * p);
    assert!(offset_of!(comline_config, framing_len) == 8 + 3 * p);
    assert!(offset_of!(comline_config, max_frame) == 8 + 4 * p);
    assert!(offset_of!(comline_config, capabilities) == 16 + 4 * p);
    assert!(offset_of!(comline_config, max_waiting) == 20 + 4 * p);
    assert!(offset_of!(comline_config, handshake) == 24 + 4 * p);
    assert!(offset_of!(comline_config, stream) == 25 + 4 * p);
    assert!(offset_of!(comline_config, unordered) == 26 + 4 * p);
    assert!(offset_of!(comline_config, _reserved) == 27 + 4 * p);
    assert!(size_of::<comline_config>() == 32 + 4 * p);
    assert!(align_of::<comline_config>() == 8);

    assert!(offset_of!(comline_comline2_config, hello) == 0);
    assert!(offset_of!(comline_comline2_config, hello_len) == p);
    assert!(offset_of!(comline_comline2_config, side) == 2 * p);
    assert!(offset_of!(comline_comline2_config, scope) == 2 * p + 1);
    assert!(offset_of!(comline_comline2_config, newer_consumer) == 2 * p + 2);
    assert!(offset_of!(comline_comline2_config, same_version_differs) == 2 * p + 3);
    assert!(offset_of!(comline_comline2_config, exchange) == 2 * p + 4);
    assert!(offset_of!(comline_comline2_config, _reserved) == 2 * p + 5);
    assert!(size_of::<comline_comline2_config>() == 2 * p + 8);

    assert!(size_of::<comline_refusal>() == 8);
    assert!(offset_of!(comline_refusal, reason) == 0);
    assert!(offset_of!(comline_refusal, side) == 1);
    assert!(offset_of!(comline_refusal, has_protocol) == 2);
    assert!(offset_of!(comline_refusal, protocol) == 4);

    assert!(size_of::<comline_protocol_outcome>() == 4);
    assert!(offset_of!(comline_protocol_outcome, number) == 0);
    assert!(offset_of!(comline_protocol_outcome, reason) == 2);

    assert!(size_of::<comline_version>() == 12);
    assert!(offset_of!(comline_version, patch) == 8);

    // The header's numbers are literals; these hold them to the core's.
    assert!(COMLINE_DEFAULT_MAX_FRAME == DEFAULT_MAX_FRAME as u64);
    assert!(COMLINE_PREFIX_LEN as usize == PREFIX_LEN);
    assert!(COMLINE_MAX_FRAME_CEILING == u32::MAX as u64 - PREFIX_LEN as u64);
    assert!(COMLINE_DEFAULT_MAX_FRAME <= COMLINE_MAX_FRAME_CEILING);
    assert!(COMLINE_REASON_MALFORMED == Reason::Malformed as u8);
    assert!(COMLINE_REASON_HANDSHAKE_VERSION == Reason::HandshakeVersion as u8);
    assert!(COMLINE_REASON_EXCHANGE_MISMATCH == Reason::ExchangeMismatch as u8);
    assert!(COMLINE_REASON_PACKAGE_MISMATCH == Reason::PackageMismatch as u8);
    assert!(COMLINE_REASON_ENCODING_MISMATCH == Reason::EncodingMismatch as u8);
    assert!(COMLINE_REASON_LINE_MISMATCH == Reason::LineMismatch as u8);
    assert!(COMLINE_REASON_SAME_VERSION_DIFFERS == Reason::SameVersionDiffers as u8);
    assert!(COMLINE_REASON_NEWER_CONSUMER == Reason::NewerConsumer as u8);
    assert!(COMLINE_REASON_UNKNOWN_PROTOCOL == Reason::UnknownProtocol as u8);
    assert!(COMLINE_SIDE_INITIATOR == Side::Initiator as u8);
    assert!(COMLINE_SIDE_RESPONDER == Side::Responder as u8);
};

// ── the connection ─────────────────────────────────────────────────────────

/// One end of a connection. Opaque: made by `comline_connection_new`, freed
/// by `comline_connection_free`.
pub struct comline_connection {
    state: State,
    framing: Framing,
    /// Our `comline@1` frame; `None` without one.
    ours: Option<HandshakeFrame>,
    /// The `comline@2` handshake and what it came to; `None` without one.
    comline2: Option<Box<Handshake2>>,
    /// Our handshake frame was written (always, without one).
    sent: bool,
    /// The peer's handshake frame arrived and agreed (always, without one).
    received: bool,
    /// Refused: a bad handshake, or an oversized length prefix.
    closed: bool,
    /// An earlier call panicked.
    poisoned: bool,
    stream: bool,
    max_frame: usize,
    /// The next call panics: the test hook for the poisoning rule.
    #[cfg(feature = "test-hooks")]
    panic_next: bool,
}

/// The connection framings, dispatched here so no generic crosses.
#[derive(Debug, Clone, Copy)]
enum Framing {
    Datagram2,
    Datagram3,
    JsonRpc,
}

/// What a `comline@2` connection reads next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Awaiting {
    Hello,
    Answer,
    Verdict,
    Nothing,
}

/// `comline@2` on a connection: [`Comline2`]'s sans-IO steps, in the order
/// [`comline_runtime::comline2::Comline2Handshake`] runs them over a transport.
struct Handshake2 {
    local: Comline2,
    side: Side,
    /// This end still has to send its hello.
    hello_due: bool,
    /// The next message to send, once the hello is out.
    pending: Option<Vec<u8>>,
    awaiting: Awaiting,
    /// What it has concluded. The peer's verdict can still refuse a protocol,
    /// or the connection, until `awaiting` is `Nothing`.
    negotiated: Option<Negotiated>,
    refusal: Option<Refusal>,
}

impl Handshake2 {
    fn new(local: Comline2, side: Side) -> Self {
        let offer = local.params.exchange == Exchange::Offer;
        let initiates = side == Side::Initiator;
        Handshake2 {
            local,
            side,
            hello_due: !offer || initiates,
            pending: None,
            awaiting: if offer && initiates {
                Awaiting::Answer
            } else {
                Awaiting::Hello
            },
            negotiated: None,
            refusal: None,
        }
    }

    /// Whether there's a message to send.
    fn has_more(&self) -> bool {
        self.hello_due || self.pending.is_some()
    }

    /// The message [`Self::has_more`] speaks of, encoded.
    fn next_message(&self) -> Option<Vec<u8>> {
        if self.hello_due {
            Some(self.local.hello_message().to_bytes())
        } else {
            self.pending.clone()
        }
    }

    /// The message [`Self::next_message`] gave was written.
    fn message_sent(&mut self) {
        if self.hello_due {
            self.hello_due = false;
        } else {
            self.pending = None;
        }
    }

    /// Take in the peer's message. Its status; a refusal is held in `refusal`.
    fn take(&mut self, frame: &[u8], ev: &mut comline_event) -> comline_status {
        let step = match self.awaiting {
            Awaiting::Hello => match self.local.params.exchange {
                Exchange::Offer => self.local.answer_offer(frame),
                Exchange::Symmetric => self.local.judge_hello(self.side, frame),
            },
            Awaiting::Answer => self.local.conclude_answer(frame),
            Awaiting::Verdict => {
                let negotiated = self
                    .negotiated
                    .take()
                    .expect("a verdict is awaited after a conclusion");
                Step {
                    reply: None,
                    result: Comline2::finish(self.side, negotiated, frame),
                }
            }
            Awaiting::Nothing => unreachable!("a finished handshake reads no more messages"),
        };
        self.awaiting = match self.awaiting {
            Awaiting::Hello => Awaiting::Verdict,
            _ => Awaiting::Nothing,
        };
        if let Some(reply) = step.reply {
            self.pending = Some(reply.to_bytes());
        }
        let status = match step.result {
            Ok(negotiated) => {
                self.negotiated = Some(negotiated);
                ev.kind = if self.awaiting == Awaiting::Nothing {
                    COMLINE_EVENT_HANDSHAKE_OK
                } else {
                    COMLINE_EVENT_HANDSHAKE_CONTINUE
                };
                COMLINE_OK
            }
            Err(refusal) => {
                // A refusal ends it: no hello after that.
                self.hello_due = false;
                self.awaiting = Awaiting::Nothing;
                self.refusal = Some(refusal);
                COMLINE_ERR_REFUSED
            }
        };
        if self.has_more() {
            ev.flags |= COMLINE_EVENT_SEND;
        }
        status
    }
}

/// Runs `$body` with `$f` bound to the framing as its concrete type.
macro_rules! with_framing {
    ($framing:expr, |$f:ident| $body:expr) => {
        match $framing {
            Framing::Datagram2 => {
                let $f = &Datagram2Framing;
                $body
            }
            Framing::Datagram3 => {
                let $f = &Datagram3Framing;
                $body
            }
            Framing::JsonRpc => {
                let $f = &JsonRpcFraming;
                $body
            }
        }
    };
}

impl comline_connection {
    fn open(&self) -> bool {
        self.sent && self.received
    }

    /// The room an encode may use in `out`, after a stream's prefix: never
    /// more than `max_frame`, so a frame over the limit overflows instead of
    /// being committed.
    fn split_out<'o>(&self, out: &'o mut [u8]) -> (&'o mut [u8], &'o mut [u8]) {
        let prefix = if self.stream { PREFIX_LEN } else { 0 };
        let (head, rest) = out.split_at_mut(prefix.min(out.len()));
        let room = rest.len().min(self.max_frame);
        (head, &mut rest[..room])
    }

    /// Finishes a frame of `len` bytes written after `head`: its prefix, in
    /// stream mode, and the total written.
    fn finish(&self, head: &mut [u8], len: usize) -> usize {
        if !self.stream {
            return len;
        }
        let mut prefix = SliceBuf::new(head);
        stream::put_prefix(len, self.max_frame, &mut prefix)
            .expect("the frame was written within max_frame");
        assert!(
            !prefix.overflowed(),
            "a frame was written without room for its prefix"
        );
        PREFIX_LEN + len
    }

    /// The status for a frame that needs `len` bytes and didn't fit: too
    /// large for the connection, or too large for the caller's buffer.
    fn too_small(&self, len: usize, written: &mut usize) -> comline_status {
        if len > self.max_frame {
            return COMLINE_ERR_FRAME_TOO_LARGE;
        }
        *written = if self.stream { PREFIX_LEN + len } else { len };
        COMLINE_BUFFER_TOO_SMALL
    }
}

/// A [`CountBuf`] that also says it overflowed, so
/// [`State::encode_request_body`] counts the whole frame and commits nothing.
struct Measure(CountBuf);

impl BufMut for Measure {
    fn put_slice(&mut self, bytes: &[u8]) {
        self.0.put_slice(bytes);
    }

    fn overflowed(&self) -> bool {
        true
    }
}

// ── the guards ─────────────────────────────────────────────────────────────

/// Runs a free function's body, turning a panic into
/// `COMLINE_ERR_POISONED`.
fn guarded(body: impl FnOnce() -> comline_status) -> comline_status {
    panic::catch_unwind(AssertUnwindSafe(body)).unwrap_or(COMLINE_ERR_POISONED)
}

/// Runs a handle's function: refuses a null or poisoned handle, and poisons
/// it if `body` panics.
///
/// # Safety
/// `conn` is null or a live handle from `comline_connection_new`, with no
/// other call on it running.
unsafe fn with_conn(
    conn: *mut comline_connection,
    body: impl FnOnce(&mut comline_connection) -> comline_status,
) -> comline_status {
    if conn.is_null() {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    if unsafe { (*conn).poisoned } {
        return COMLINE_ERR_POISONED;
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller's contract; this is the only reference.
        let conn = unsafe { &mut *conn };
        #[cfg(feature = "test-hooks")]
        if core::mem::take(&mut conn.panic_next) {
            panic!("comline_test_panic_next");
        }
        if conn.closed {
            return COMLINE_ERR_STATE;
        }
        body(conn)
    }));
    match result {
        Ok(status) => status,
        Err(_) => {
            // SAFETY: the reference made inside the closure is gone.
            unsafe { (*conn).poisoned = true };
            COMLINE_ERR_POISONED
        }
    }
}

/// [`with_conn`], for what also holds on a closed handle: reading why it closed.
///
/// # Safety
/// As [`with_conn`].
unsafe fn with_closed_too(
    conn: *mut comline_connection,
    body: impl FnOnce(&mut comline_connection) -> comline_status,
) -> comline_status {
    if conn.is_null() {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    if unsafe { (*conn).poisoned } {
        return COMLINE_ERR_POISONED;
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller's contract; this is the only reference.
        body(unsafe { &mut *conn })
    }));
    result.unwrap_or_else(|_| {
        // SAFETY: the reference made inside the closure is gone.
        unsafe { (*conn).poisoned = true };
        COMLINE_ERR_POISONED
    })
}

/// `len` bytes at `ptr`, which may be null (or dangling) when `len` is 0.
///
/// # Safety
/// Unless null, `ptr` points to `len` readable bytes that stay unchanged
/// for `'a`.
unsafe fn input<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len > isize::MAX as usize {
        None
    } else if len == 0 {
        Some(&[])
    } else if ptr.is_null() {
        None
    } else {
        // SAFETY: the caller's contract.
        Some(unsafe { slice::from_raw_parts(ptr, len) })
    }
}

/// `cap` writable bytes at `ptr`, which may be null when `cap` is 0.
///
/// # Safety
/// Unless null, `ptr` points to `cap` writable bytes nothing else uses for
/// `'a`.
unsafe fn output<'a>(ptr: *mut u8, cap: usize) -> Option<&'a mut [u8]> {
    if cap > isize::MAX as usize {
        None
    } else if cap == 0 {
        Some(&mut [])
    } else if ptr.is_null() {
        None
    } else {
        // SAFETY: the caller's contract.
        Some(unsafe { slice::from_raw_parts_mut(ptr, cap) })
    }
}

/// Memory one call reads or writes: the addresses `start..end`. Empty when
/// `end == start`.
#[derive(Clone, Copy)]
struct Region {
    start: usize,
    end: usize,
}

/// `len` values of `T` at `ptr`. A null pointer is no memory: the call
/// refuses it on its own. A length past the address space ends at its top
/// (the call refuses any length over `isize::MAX` on its own too).
#[inline(always)]
fn region<T>(ptr: *const T, len: usize) -> Region {
    let start = ptr as usize;
    let len = len.saturating_mul(core::mem::size_of::<T>());
    Region {
        start,
        end: if start == 0 {
            0
        } else {
            start.saturating_add(len)
        },
    }
}

/// Whether `a` and `b` share a byte: the later start is before the earlier
/// end, which an empty region's never is.
#[inline(always)]
fn overlap(a: Region, b: Region) -> bool {
    a.start.max(b.start) < a.end.min(b.end)
}

/// Whether no memory a call writes shares a byte with any other memory it
/// takes. A `&mut` that aliases another reference is undefined behaviour,
/// so this is checked on addresses alone, before any reference is made.
///
/// `disjoint!(writes [(ptr, len), ...], reads [(ptr, len), ...])`, lengths
/// counted in the pointer's type. Every call runs it, so it expands to the
/// pairs themselves, without a loop: each written region against every
/// region after it, and no pair of two reads.
macro_rules! disjoint {
    (writes [$($w:expr),+ $(,)?], reads [$($r:expr),* $(,)?]) => {
        !disjoint!(@shared [$(region($w.0, $w.1)),+] [$(region($r.0, $r.1)),*])
    };
    (writes [$($w:expr),+ $(,)?]) => {
        disjoint!(writes [$($w),+], reads [])
    };
    (@shared [$w:expr] [$($r:expr),*]) => {
        false $(| overlap($w, $r))*
    };
    (@shared [$w:expr, $($ws:expr),+] [$($r:expr),*]) => {
        false $(| overlap($w, $ws))* $(| overlap($w, $r))*
            | disjoint!(@shared [$($ws),+] [$($r),*])
    };
}

/// UTF-8 text of `len` bytes at `ptr`.
///
/// # Safety
/// As [`input`].
unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    // SAFETY: the caller's contract.
    core::str::from_utf8(unsafe { input(ptr, len) }?).ok()
}

/// Where `part` lies in `base`, or `None` when it isn't inside it (a
/// JSON-RPC body that was absent, which the framing gives as a static
/// `null`).
fn span_in(base: &[u8], part: &[u8]) -> Option<comline_span> {
    let start = (part.as_ptr() as usize).checked_sub(base.as_ptr() as usize)?;
    let end = start.checked_add(part.len())?;
    if end > base.len() {
        return None;
    }
    // `start` and the length are each at most `end`, so one check covers
    // both casts. It never fails: a frame starts at 0 or after its prefix
    // and is at most `max_frame`, which is at most the ceiling, so anything
    // in it ends by `PREFIX_LEN + COMLINE_MAX_FRAME_CEILING == u32::MAX`.
    u32::try_from(end).expect("spans fit u32 under the frame ceiling");
    Some(comline_span {
        offset: start as u32,
        len: part.len() as u32,
    })
}

/// A body's span and, when the frame had none, `COMLINE_EVENT_BODY_ABSENT`.
fn body_span(base: &[u8], body: &[u8]) -> (comline_span, u8) {
    match span_in(base, body) {
        Some(span) => (span, 0),
        None => (comline_span::default(), COMLINE_EVENT_BODY_ABSENT),
    }
}

/// A method name's span as the frame spells it, and
/// `COMLINE_EVENT_NAME_ESCAPED` when that spelling has escapes.
fn name_span(base: &[u8], name: MethodName<'_>) -> (comline_span, u8) {
    let (written, escaped) = name.as_written();
    let span = span_in(base, written.as_bytes()).expect("a name borrows the frame");
    (
        span,
        if escaped {
            COMLINE_EVENT_NAME_ESCAPED
        } else {
            0
        },
    )
}

// ── making and freeing a connection ────────────────────────────────────────

/// Make a connection from `cfg`, into `*out`. The one allocation the library
/// makes: the handle, with room for `max_waiting` waiting calls. (One known
/// exception, a bug: `comline_connection_feed` on `jsonrpc@1`; see there.)
///
/// Errors: `COMLINE_ERR_INVALID_ARGUMENT` (a null pointer, a name that isn't
/// UTF-8, a field outside its values, `max_frame` above the ceiling,
/// `*out` overlapping the config or its names) or
/// `COMLINE_ERR_UNKNOWN_FRAMING`. `*out` is null on any error, unless it
/// overlaps an input, when it is left as it was.
///
/// # Safety
/// `cfg` is null or a valid `comline_config` whose name pointers satisfy
/// their lengths; `out` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_new(
    cfg: *const comline_config,
    out: *mut *mut comline_connection,
) -> comline_status {
    // SAFETY: the caller's contract.
    unsafe { connection_new(cfg, ptr::null(), out) }
}

/// [`comline_connection_new`] for a `comline@2` connection: `cfg->handshake`
/// must be `COMLINE_HANDSHAKE_COMLINE2`, and `comline2` holds its facts and
/// parameters. `cfg->ir_hash` and `cfg->capabilities` aren't read: `comline@2`
/// carries neither.
///
/// Errors: as `comline_connection_new`, and `COMLINE_ERR_INVALID_ARGUMENT` for
/// a `hello` that isn't an encoded hello message or a field outside its values.
///
/// # Safety
/// As `comline_connection_new`; `comline2` is null or a valid
/// `comline_comline2_config` whose `hello` satisfies `hello_len`.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_new_comline2(
    cfg: *const comline_config,
    comline2: *const comline_comline2_config,
    out: *mut *mut comline_connection,
) -> comline_status {
    if comline2.is_null() {
        if !out.is_null() {
            // SAFETY: checked non-null; the caller's contract.
            unsafe { out.write(ptr::null_mut()) };
        }
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe { connection_new(cfg, comline2, out) }
}

/// Makes a connection, with `comline2` for a `comline@2` one (else null).
///
/// # Safety
/// As `comline_connection_new_comline2`.
unsafe fn connection_new(
    cfg: *const comline_config,
    comline2: *const comline_comline2_config,
    out: *mut *mut comline_connection,
) -> comline_status {
    guarded(|| {
        if out.is_null() {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        if cfg.is_null() {
            // SAFETY: checked non-null; the caller's contract.
            unsafe { out.write(ptr::null_mut()) };
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        if !disjoint!(writes[(out, 1)], reads[(cfg, 1), (comline2, 1)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null and apart; the caller's contract.
        let cfg = unsafe { &*cfg };
        if !disjoint!(
            writes [(out, 1)],
            reads [
                (cfg.framing, cfg.framing_len),
                (cfg.wire_format, cfg.wire_format_len),
            ]
        ) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null and apart; the caller's contract.
        unsafe { out.write(ptr::null_mut()) };
        // SAFETY: the caller's contract.
        let (Some(framing_name), Some(wire_format)) = (unsafe {
            (
                text(cfg.framing, cfg.framing_len),
                text(cfg.wire_format, cfg.wire_format_len),
            )
        }) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let flag = |v: u8| match v {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        let (Some(stream), Some(unordered)) = (flag(cfg.stream), flag(cfg.unordered)) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        if cfg._reserved != 0 || cfg.max_frame > COMLINE_MAX_FRAME_CEILING {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        let framing = match framing_name {
            FRAMING_DATAGRAM2 => Framing::Datagram2,
            FRAMING_DATAGRAM3 => Framing::Datagram3,
            FRAMING_JSONRPC => Framing::JsonRpc,
            _ => return COMLINE_ERR_UNKNOWN_FRAMING,
        };
        let ours = match cfg.handshake {
            COMLINE_HANDSHAKE_COMLINE if comline2.is_null() => Some(HandshakeFrame::new(
                cfg.ir_hash,
                wire_format,
                framing_name,
                cfg.capabilities,
            )),
            COMLINE_HANDSHAKE_NONE if comline2.is_null() => None,
            COMLINE_HANDSHAKE_COMLINE2 if !comline2.is_null() => None,
            _ => return COMLINE_ERR_INVALID_ARGUMENT,
        };
        let comline2 = if comline2.is_null() {
            None
        } else {
            // SAFETY: checked non-null; the caller's contract.
            let c = unsafe { &*comline2 };
            if !disjoint!(writes[(out, 1)], reads[(c.hello, c.hello_len)]) {
                return COMLINE_ERR_INVALID_ARGUMENT;
            }
            // SAFETY: the caller's contract.
            let Some(hello) = (unsafe { input(c.hello, c.hello_len) }) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            let Some(handshake) = handshake2(c, hello) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            Some(Box::new(handshake))
        };
        let settled = ours.is_none() && comline2.is_none();
        let max_frame = match cfg.max_frame {
            0 => DEFAULT_MAX_FRAME,
            // At most the ceiling, so it fits a `usize` on every target.
            n => n as usize,
        };
        let capacity = cfg.max_waiting as usize;
        let state = if unordered {
            State::unordered_with_capacity(capacity)
        } else {
            State::with_capacity(capacity)
        };
        let conn = Box::new(comline_connection {
            state,
            framing,
            ours,
            comline2,
            sent: settled,
            received: settled,
            closed: false,
            poisoned: false,
            stream,
            max_frame,
            #[cfg(feature = "test-hooks")]
            panic_next: false,
        });
        // SAFETY: checked non-null.
        unsafe { out.write(Box::into_raw(conn)) };
        COMLINE_OK
    })
}

/// Free a connection. Null is a no-op. Works on a poisoned or closed handle.
///
/// # Safety
/// `conn` is null or a handle from `comline_connection_new`, not freed
/// before, with no other call on it running. It is invalid afterwards.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_free(conn: *mut comline_connection) {
    // Not through `with_conn`, which refuses a poisoned or closed handle:
    // those are exactly the ones a binding frees. Freeing one is sound. A
    // panic unwinds out of safe code only, which leaves every field (plain
    // integers, flags, a `Vec<u64>`) valid, so it drops like any other.
    if conn.is_null() {
        return;
    }
    // Dropping holds no user code; a panic here would be a bug in `Vec`.
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller's contract.
        drop(unsafe { Box::from_raw(conn) });
    }));
}

/// A [`Handshake2`] from its config, or `None` for a field outside its values.
fn handshake2(c: &comline_comline2_config, hello: &[u8]) -> Option<Handshake2> {
    if c._reserved != [0; 3] {
        return None;
    }
    let Ok(Message::Hello(hello)) = Message::decode(hello) else {
        return None;
    };
    let params = Params {
        scope: match c.scope {
            COMLINE_SCOPE_PROTOCOL => Scope::Protocol,
            COMLINE_SCOPE_PACKAGE => Scope::Package,
            _ => return None,
        },
        newer_consumer: match c.newer_consumer {
            COMLINE_NEWER_CONSUMER_DEGRADE => NewerConsumer::Degrade,
            COMLINE_NEWER_CONSUMER_REFUSE => NewerConsumer::Refuse,
            _ => return None,
        },
        same_version_differs: match c.same_version_differs {
            COMLINE_SAME_VERSION_DIFFERS_REFUSE => SameVersionDiffers::Refuse,
            COMLINE_SAME_VERSION_DIFFERS_WARN => SameVersionDiffers::Warn,
            _ => return None,
        },
        exchange: match c.exchange {
            COMLINE_EXCHANGE_OFFER => Exchange::Offer,
            COMLINE_EXCHANGE_SYMMETRIC => Exchange::Symmetric,
            _ => return None,
        },
    };
    Some(Handshake2::new(
        Comline2::new(params, hello),
        Side::from_code(c.side)?,
    ))
}

// ── the handshake on a connection ──────────────────────────────────────────

/// Write this side's handshake frame (with its length prefix in stream
/// mode), to send before anything else. Without a handshake it writes
/// nothing and returns `COMLINE_OK`.
///
/// The peer's frame is the first one fed (`COMLINE_EVENT_HANDSHAKE_OK`), and
/// may arrive before or after this call. Requests and replies need both.
///
/// **`comline@2`** has several messages, so call this again whenever an event
/// or a refusal says `COMLINE_EVENT_SEND`, and once at the start. It writes the
/// next message, or nothing (`*written` 0, `COMLINE_OK`) when there's none: under
/// `exchange = offer` the responder has nothing to send until it has read the
/// hello. A second call is therefore no `COMLINE_ERR_STATE` here. Requests and
/// replies need every message written and the handshake finished.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` = 31, or 35 in stream
/// mode; for `comline@2`, the message's size), `COMLINE_ERR_FRAME_TOO_LARGE`
/// (`max_frame` below 31, or below that message),
/// `COMLINE_ERR_STATE` when it was already written.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable bytes;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_start(
    conn: *mut comline_connection,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(out), Some(written)) = (output(out, cap), written.as_mut()) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *written = 0;
            if conn.comline2.is_some() {
                return start_comline2(conn, out, written);
            }
            let Some(ours) = conn.ours else {
                return COMLINE_OK;
            };
            if conn.sent {
                return COMLINE_ERR_STATE;
            }
            let len = COMLINE_HANDSHAKE_LEN as usize;
            let (head, room) = conn.split_out(out);
            let mut buf = SliceBuf::new(room);
            ours.encode(&mut buf);
            if buf.overflowed() {
                return conn.too_small(len, written);
            }
            *written = conn.finish(head, len);
            conn.sent = true;
            COMLINE_OK
        })
    }
}

/// [`comline_connection_start`] on a `comline@2` connection.
#[cold]
#[inline(never)]
fn start_comline2(
    conn: &mut comline_connection,
    out: &mut [u8],
    written: &mut usize,
) -> comline_status {
    let handshake = conn.comline2.as_ref().expect("the caller checked");
    let Some(message) = handshake.next_message() else {
        return COMLINE_OK;
    };
    let (head, room) = conn.split_out(out);
    if message.len() > room.len() {
        return conn.too_small(message.len(), written);
    }
    room[..message.len()].copy_from_slice(&message);
    *written = conn.finish(head, message.len());
    let handshake = conn.comline2.as_mut().expect("the caller checked");
    handshake.message_sent();
    let more = handshake.has_more();
    let refused = handshake.refusal.is_some();
    conn.sent = !more;
    if refused && !more {
        conn.closed = true;
    }
    COMLINE_OK
}

/// [`feed`]'s frame while a `comline@2` handshake isn't finished. Out of line,
/// so a handshake that runs once costs [`feed`]'s every frame nothing.
#[cold]
#[inline(never)]
fn feed_comline2(
    conn: &mut comline_connection,
    frame: &[u8],
    consumed: &mut usize,
    ev: &mut comline_event,
) -> comline_status {
    let handshake = conn.comline2.as_mut().expect("the caller checked");
    // A refusal only has its last message to send, and an offer goes first.
    let offering = handshake.hello_due && handshake.awaiting == Awaiting::Answer;
    if handshake.refusal.is_some() || offering {
        *consumed = 0;
        ev.frame = comline_span::default();
        return COMLINE_ERR_STATE;
    }
    let status = handshake.take(frame, ev);
    let more = handshake.has_more();
    let refused = handshake.refusal.is_some();
    let finished = handshake.awaiting == Awaiting::Nothing;
    conn.sent = !more;
    conn.received = finished && !refused;
    if refused && !more {
        conn.closed = true;
    }
    status
}

// ── feeding frames ─────────────────────────────────────────────────────────

/// Feed received bytes and get the event for the next frame. One frame per
/// call.
///
/// - **Stream mode:** `in` is whatever the host has read so far. If a whole
///   length-prefixed frame is at its start, it is decoded, `*consumed` is
///   the prefix plus the frame, and `*ev` describes it, with spans relative
///   to `in`. Bytes after it are left for the next call. Otherwise
///   `COMLINE_NEED_MORE`, with `*consumed` 0 and `ev->frame.len` the total
///   needed once the prefix is complete (0 before).
/// - **Message mode:** `in` is exactly one frame, and `*consumed` is its
///   length.
///
/// On a `comline@1` connection the first frame is the peer's handshake:
/// `COMLINE_EVENT_HANDSHAKE_OK`, or `COMLINE_ERR_HANDSHAKE` and the
/// connection closes.
///
/// On a `comline@2` connection the first frames are the handshake's:
/// `COMLINE_EVENT_HANDSHAKE_CONTINUE` while it goes on,
/// `COMLINE_EVENT_HANDSHAKE_OK` when it's done, or `COMLINE_ERR_REFUSED`
/// (`comline_connection_refusal`). `COMLINE_EVENT_SEND` on any of them means
/// `comline_connection_start` has a message to send. Fed while its own offer
/// hasn't been written, or after a refusal, it's `COMLINE_ERR_STATE` with
/// nothing consumed.
///
/// After that, a frame is a request, a reply to a waiting call (which then
/// waits no more), or a stale reply. A malformed frame is
/// `COMLINE_ERR_FRAMING` with the frame consumed. In stream mode, a prefix
/// over `max_frame` is `COMLINE_ERR_FRAME_TOO_LARGE` and closes the
/// connection.
///
/// **What each framing reads:**
/// - `datagram@2`: the protocol number (`COMLINE_EVENT_HAS_PROTOCOL`), the
///   `call_id` and the request id. It can't mark a call one-way.
/// - `datagram@3`: the same, the protocol and `call_id` being wire numbers,
///   each written as a shortest-form varint.
/// - `jsonrpc@1`: the method (`COMLINE_EVENT_CALL_BY_NAME`; with
///   `COMLINE_EVENT_NAME_ESCAPED` when the frame spelled it with escapes) and
///   the `id`. A notification is `COMLINE_EVENT_ONE_WAY` with request id 0.
///   Absent `params`, `result` or `error.data` is
///   `COMLINE_EVENT_BODY_ABSENT`.
///
/// **Known bug (runtime#58):** on `jsonrpc@1` this call can allocate, which
/// breaks the rule that nothing allocates after `comline_connection_new`.
/// The JSON parser allocates for any refused frame, for a member name
/// written with escapes, and for brackets nested three deep in the frame
/// (`"params":[[1]]`, counting the frame's own `{`). `datagram@2` never
/// allocates here.
///
/// # Safety
/// `conn` is null or a live handle; `in` is null or `in_len` readable bytes;
/// `consumed` and `ev` are null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_feed(
    conn: *mut comline_connection,
    in_: *const u8,
    in_len: usize,
    consumed: *mut usize,
    ev: *mut comline_event,
) -> comline_status {
    if !disjoint!(
        writes [(conn, 1), (consumed, 1), (ev, 1)],
        reads [(in_, in_len)]
    ) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(input), Some(consumed), Some(ev)) =
                (input(in_, in_len), consumed.as_mut(), ev.as_mut())
            else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *consumed = 0;
            *ev = comline_event::default();
            feed(conn, input, consumed, ev)
        })
    }
}

fn feed(
    conn: &mut comline_connection,
    input: &[u8],
    consumed: &mut usize,
    ev: &mut comline_event,
) -> comline_status {
    let (start, len) = if conn.stream {
        match stream::split_frame(input, conn.max_frame) {
            Split::Frame { start, len } => (start, len),
            Split::NeedMore { needed } => {
                ev.frame.len = u32::try_from(needed).expect("a prefix is at most the ceiling");
                return COMLINE_NEED_MORE;
            }
            Split::TooLarge => {
                conn.closed = true;
                return COMLINE_ERR_FRAME_TOO_LARGE;
            }
        }
    } else {
        if input.len() > conn.max_frame {
            return COMLINE_ERR_FRAME_TOO_LARGE;
        }
        (0, input.len())
    };
    let frame = &input[start..start + len];
    *consumed = start + len;
    ev.frame = span_in(input, frame).expect("the frame is in the input");

    if !conn.received {
        if conn.comline2.is_some() {
            return feed_comline2(conn, frame, consumed, ev);
        }
        let ours = conn.ours.expect("only a handshake leaves `received` unset");
        return match ours.check_bytes(frame) {
            Ok(peer) => {
                conn.received = true;
                ev.kind = COMLINE_EVENT_HANDSHAKE_OK;
                ev.capabilities = peer.capabilities;
                COMLINE_OK
            }
            Err(error) => {
                conn.closed = true;
                status_of(error)
            }
        };
    }

    let event = match with_framing!(conn.framing, |f| conn.state.receive(f, frame)) {
        Ok(event) => event,
        Err(error) => return status_of(error),
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
                    let (span, escaped) = name_span(input, name);
                    ev.flags |= COMLINE_EVENT_CALL_BY_NAME | escaped;
                    ev.call_name = span;
                }
            }
            ev.request_id = request.request_id;
            let (body, absent) = body_span(input, request.params);
            ev.body = body;
            ev.flags |= absent;
        }
        Event::Reply {
            request_id,
            envelope,
        } => {
            ev.kind = COMLINE_EVENT_REPLY;
            ev.request_id = request_id;
            let body = match envelope {
                Envelope::Ok(payload) => {
                    ev.envelope = COMLINE_ENVELOPE_OK;
                    Some(payload)
                }
                Envelope::Err { id, body } => {
                    ev.envelope = COMLINE_ENVELOPE_ERR;
                    ev.error_id = id;
                    Some(body)
                }
                Envelope::Fault => {
                    ev.envelope = COMLINE_ENVELOPE_FAULT;
                    None
                }
            };
            if let Some(body) = body {
                let (body, absent) = body_span(input, body);
                ev.body = body;
                ev.flags |= absent;
            }
        }
        Event::Stale => ev.kind = COMLINE_EVENT_STALE,
    }
    COMLINE_OK
}

// ── encoding frames ────────────────────────────────────────────────────────

/// Frame a request around a body the host's codec already wrote, with its
/// length prefix in stream mode, so `out` is ready for one transport write.
/// `*request_id` is the id it was given. A call that `expects_reply` (1)
/// waits until its reply is fed or it is abandoned; a one-way call (0) does
/// not wait.
///
/// **What each framing reads:**
/// - `datagram@2`: `protocol` and `call_id`; `name` is ignored. The frame
///   can't say one-way.
/// - `datagram@3`: the same, `protocol` and `call_id` being wire numbers.
/// - `jsonrpc@1`: `name`, escaped as a JSON string as serde_json escapes
///   one; `protocol` and `call_id` are ignored. A one-way call is a
///   notification, with no `id`. An empty body is written as `null`.
///
/// `name` is any UTF-8 text (it may be empty for `datagram@2`).
///
/// Errors, none of which spend a request id or leave a call waiting:
/// - `COMLINE_BUFFER_TOO_SMALL`: `*written` is the exact size needed;
/// - `COMLINE_ERR_FRAME_TOO_LARGE`: the frame is over `max_frame`;
/// - `COMLINE_ERR_BUSY`: `max_waiting` calls already wait;
/// - `COMLINE_ERR_INVALID_ARGUMENT`: a name that isn't UTF-8,
///   `expects_reply` not 0 or 1, or a null pointer;
/// - `COMLINE_ERR_STATE`: the handshake hasn't completed.
///
/// # Safety
/// `conn` is null or a live handle; `name`, `body` are null or readable for
/// their lengths; `out` is null or `cap` writable bytes; `written` and
/// `request_id` are null or writable.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn comline_connection_encode_request(
    conn: *mut comline_connection,
    protocol: u16,
    call_id: u16,
    name: *const u8,
    name_len: usize,
    expects_reply: u8,
    body: *const u8,
    body_len: usize,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
    request_id: *mut u64,
) -> comline_status {
    if !disjoint!(
        writes [(conn, 1), (out, cap), (written, 1), (request_id, 1)],
        reads [(name, name_len), (body, body_len)]
    ) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(body), Some(out), Some(written), Some(request_id)) = (
                input(body, body_len),
                output(out, cap),
                written.as_mut(),
                request_id.as_mut(),
            ) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *written = 0;
            let Some(name) = text(name, name_len) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            let expects_reply = match expects_reply {
                0 => false,
                1 => true,
                _ => return COMLINE_ERR_INVALID_ARGUMENT,
            };
            if !conn.open() {
                return COMLINE_ERR_STATE;
            }
            let framing = conn.framing;
            let (head, room) = conn.split_out(out);
            let mut buf = SliceBuf::new(room);
            let encoded = with_framing!(framing, |f| conn.state.encode_request_body(
                f,
                protocol,
                call_id,
                name,
                body,
                expects_reply,
                &mut buf,
            ));
            match encoded {
                Ok(id) => {
                    let len = buf.written().len();
                    *written = conn.finish(head, len);
                    *request_id = id;
                    COMLINE_OK
                }
                Err(RuntimeError::BufferTooSmall) => {
                    // Count the frame without committing it.
                    let mut measure = Measure(CountBuf::new());
                    let again = with_framing!(framing, |f| conn.state.encode_request_body(
                        f,
                        protocol,
                        call_id,
                        name,
                        body,
                        expects_reply,
                        &mut measure,
                    ));
                    debug_assert_eq!(again, Err(RuntimeError::BufferTooSmall));
                    conn.too_small(measure.0.len(), written)
                }
                Err(error) => status_of(error),
            }
        })
    }
}

/// Frame the reply to a request this side was given, with its length prefix
/// in stream mode. `envelope` is `COMLINE_ENVELOPE_OK` (`body` is the
/// result), `COMLINE_ENVELOPE_ERR` (`error_id` is the schema-global ordinal,
/// `body` the error's fields) or `COMLINE_ENVELOPE_FAULT` (no body:
/// `body_len` must be 0).
///
/// **What each framing reads:** `datagram@2` and `datagram@3` write `protocol`
/// (the one the request was for) and the request id; `jsonrpc@1` ignores `protocol`,
/// writes an empty body as `null`, an error as a JSON-RPC `error` object with
/// `code` = `error_id`, and a fault as `-32603`.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is the exact size needed),
/// `COMLINE_ERR_FRAME_TOO_LARGE`, `COMLINE_ERR_INVALID_ARGUMENT`,
/// `COMLINE_ERR_STATE` (the handshake hasn't completed).
///
/// # Safety
/// `conn` is null or a live handle; `body` is null or readable for
/// `body_len`; `out` is null or `cap` writable bytes; `written` is null or
/// writable.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn comline_connection_encode_reply(
    conn: *mut comline_connection,
    protocol: u16,
    request_id: u64,
    envelope: u8,
    error_id: u16,
    body: *const u8,
    body_len: usize,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(
        writes [(conn, 1), (out, cap), (written, 1)],
        reads [(body, body_len)]
    ) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(body), Some(out), Some(written)) =
                (input(body, body_len), output(out, cap), written.as_mut())
            else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *written = 0;
            let envelope = match envelope {
                COMLINE_ENVELOPE_OK => Envelope::Ok(body),
                COMLINE_ENVELOPE_ERR => Envelope::Err { id: error_id, body },
                COMLINE_ENVELOPE_FAULT if body.is_empty() => Envelope::Fault,
                _ => return COMLINE_ERR_INVALID_ARGUMENT,
            };
            if !conn.open() {
                return COMLINE_ERR_STATE;
            }
            let (head, room) = conn.split_out(out);
            let mut buf = SliceBuf::new(room);
            with_framing!(conn.framing, |f| f
                .encode_reply(protocol, request_id, &envelope, &mut buf));
            if buf.overflowed() {
                let mut count = CountBuf::new();
                with_framing!(conn.framing, |f| f
                    .encode_reply(protocol, request_id, &envelope, &mut count));
                return conn.too_small(count.len(), written);
            }
            let len = buf.written().len();
            *written = conn.finish(head, len);
            COMLINE_OK
        })
    }
}

// ── waiting calls ──────────────────────────────────────────────────────────

/// Stop waiting on `request_id` (it timed out; the host's clock decides
/// when). A late reply to it is then `COMLINE_EVENT_STALE`. An id nobody
/// waits on is a no-op.
///
/// # Safety
/// `conn` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_abandon(
    conn: *mut comline_connection,
    request_id: u64,
) -> comline_status {
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            conn.state.abandon(request_id);
            COMLINE_OK
        })
    }
}

/// Whether any call waits on a reply: `*waiting` is 1 or 0.
///
/// # Safety
/// `conn` is null or a live handle; `waiting` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_is_waiting(
    conn: *mut comline_connection,
    waiting: *mut u8,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (waiting, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(waiting) = waiting.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *waiting = u8::from(conn.state.is_waiting());
            COMLINE_OK
        })
    }
}

// ── comline@2 ──────────────────────────────────────────────────────────────

/// Writes `items` to the `cap` values at `out`: `*written` is how many there
/// are, and `COMLINE_BUFFER_TOO_SMALL` when they don't fit, with nothing
/// written.
///
/// # Safety
/// `out` is null or `cap` writable, aligned values of `T`; `written` is null
/// or writable; neither overlaps `items`.
unsafe fn list_out<T: Copy>(
    items: &[T],
    out: *mut T,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    // SAFETY: the caller's contract.
    let Some(written) = (unsafe { written.as_mut() }) else {
        return COMLINE_ERR_INVALID_ARGUMENT;
    };
    if cap.saturating_mul(core::mem::size_of::<T>()) > isize::MAX as usize
        || (out.is_null() && cap != 0)
    {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    *written = items.len();
    if items.len() > cap {
        return COMLINE_BUFFER_TOO_SMALL;
    }
    if !items.is_empty() {
        // SAFETY: `items.len() <= cap`, which the caller's contract makes room
        // for, so `out` isn't null, and it doesn't overlap `items`.
        unsafe { ptr::copy_nonoverlapping(items.as_ptr(), out, items.len()) };
    }
    COMLINE_OK
}

/// What the handshake has concluded, for a function that reads it.
fn negotiated(conn: &comline_connection) -> Option<&Negotiated> {
    conn.comline2.as_ref()?.negotiated.as_ref()
}

/// Whether this end may call `function` of `protocol`, both wire numbers:
/// `COMLINE_OK` to send it, or `COMLINE_ERR_UNAVAILABLE` when the peer lacks the
/// function or the protocol was refused. Call it before encoding a request,
/// so a call the peer can't take never reaches the wire.
///
/// On a connection without `comline@2` it's always `COMLINE_OK`: there nothing
/// is negotiated per function. Before the handshake concluded, or after it
/// refused, it's `COMLINE_ERR_STATE`.
///
/// # Safety
/// `conn` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_check_call(
    conn: *mut comline_connection,
    protocol: u16,
    function: u16,
) -> comline_status {
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            if conn.comline2.is_none() {
                return COMLINE_OK;
            }
            match negotiated(conn) {
                None => COMLINE_ERR_STATE,
                Some(n) => match n.check_call(protocol, function) {
                    Ok(()) => COMLINE_OK,
                    Err(error) => status_of(error),
                },
            }
        })
    }
}

/// Why `comline@2` refused this connection, into `*refusal`, after
/// `COMLINE_ERR_REFUSED`. It works on the closed handle a refusal leaves.
///
/// Errors: `COMLINE_ERR_STATE` when nothing was refused.
///
/// # Safety
/// `conn` is null or a live handle; `refusal` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_refusal(
    conn: *mut comline_connection,
    refusal: *mut comline_refusal,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (refusal, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_closed_too(conn, |conn| {
            let Some(refusal) = refusal.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *refusal = comline_refusal::default();
            let Some(r) = conn.comline2.as_ref().and_then(|h| h.refusal) else {
                return COMLINE_ERR_STATE;
            };
            refusal.reason = r.reason.code();
            refusal.side = r.side.code();
            if let Some(protocol) = r.protocol {
                refusal.has_protocol = 1;
                refusal.protocol = protocol;
            }
            COMLINE_OK
        })
    }
}

/// The wire numbers of the protocols that connected although a shape differs
/// at the same package version (`same_version_differs = warn`), ascending, to
/// report to whoever runs this: one end isn't what its version claims. `cap`
/// and `*written` count numbers.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is how many there are),
/// `COMLINE_ERR_STATE` before the handshake concluded, after it refused, or
/// without `comline@2`.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable numbers;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_warnings(
    conn: *mut comline_connection,
    out: *mut u16,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| match negotiated(conn) {
            None => COMLINE_ERR_STATE,
            // SAFETY: the caller's contract.
            Some(n) => list_out(&n.warnings, out, cap, written),
        })
    }
}

/// What each of this end's protocols came to, ascending by number. `cap` and
/// `*written` count outcomes. It holds what this end concluded, and the peer's
/// verdict can still refuse a protocol until `COMLINE_EVENT_HANDSHAKE_OK`.
///
/// Errors: as `comline_connection_warnings`.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable outcomes;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_protocols(
    conn: *mut comline_connection,
    out: *mut comline_protocol_outcome,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(n) = negotiated(conn) else {
                return COMLINE_ERR_STATE;
            };
            let Some(written) = written.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            let size = cap.saturating_mul(core::mem::size_of::<comline_protocol_outcome>());
            if size > isize::MAX as usize || (out.is_null() && cap != 0) {
                return COMLINE_ERR_INVALID_ARGUMENT;
            }
            *written = n.protocols.len();
            if n.protocols.len() > cap {
                return COMLINE_BUFFER_TOO_SMALL;
            }
            for (i, p) in n.protocols.iter().enumerate() {
                let reason = match &p.state {
                    ProtocolState::Accepted { .. } => 0,
                    ProtocolState::Refused { reason } => reason.code(),
                };
                let outcome = comline_protocol_outcome {
                    number: p.number,
                    reason,
                    _reserved: 0,
                };
                // SAFETY: `i < cap`, which the caller's contract makes room for.
                out.add(i).write(outcome);
            }
            COMLINE_OK
        })
    }
}

/// The functions of `protocol` the peer lacks, ascending by number: those
/// `comline_connection_check_call` refuses. `cap` and `*written` count numbers.
/// A protocol this end doesn't have, or one refused, lacks none.
///
/// Errors: as `comline_connection_warnings`.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable numbers;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_peer_lacks(
    conn: *mut comline_connection,
    protocol: u16,
    out: *mut u16,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(n) = negotiated(conn) else {
                return COMLINE_ERR_STATE;
            };
            let lacks = n
                .protocols
                .iter()
                .find(|p| p.number == protocol)
                .and_then(|p| match &p.state {
                    ProtocolState::Accepted { peer_lacks } => Some(peer_lacks.as_slice()),
                    ProtocolState::Refused { .. } => None,
                });
            // SAFETY: the caller's contract.
            list_out(lacks.unwrap_or(&[]), out, cap, written)
        })
    }
}

/// The peer's package version: its release in `*version`, and its pre-release
/// (`dev.2`, empty for a release) as UTF-8 in `pre`. `cap` and `*written` count
/// the pre-release's bytes.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is the size needed, and
/// `*version` is set), and as `comline_connection_warnings`.
///
/// # Safety
/// `conn` is null or a live handle; `version` and `written` are null or
/// writable; `pre` is null or `cap` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_peer_version(
    conn: *mut comline_connection,
    version: *mut comline_version,
    pre: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (version, 1), (pre, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(version) = version.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *version = comline_version::default();
            let Some(n) = negotiated(conn) else {
                return COMLINE_ERR_STATE;
            };
            let release = n.peer_version.release;
            *version = comline_version {
                major: release.major,
                minor: release.minor,
                patch: release.patch,
            };
            // SAFETY: the caller's contract.
            list_out(n.peer_version.pre.as_bytes(), pre, cap, written)
        })
    }
}

/// Read one `comline@2` message: `COMLINE_OK` and its type in `*kind`
/// (`COMLINE_COMLINE2_*`), or `COMLINE_ERR_HANDSHAKE` and why it can't be read
/// in `*reason` (`COMLINE_REASON_MALFORMED`, or
/// `COMLINE_REASON_HANDSHAKE_VERSION` for another version's frame). The other
/// is 0. For a tool or a test: a connection reads its messages itself.
///
/// # Safety
/// `in` is null or readable for `len`; `kind` and `reason` are null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_comline2_message_check(
    in_: *const u8,
    len: usize,
    kind: *mut u8,
    reason: *mut u8,
) -> comline_status {
    guarded(|| {
        if !disjoint!(writes [(kind, 1), (reason, 1)], reads [(in_, len)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked apart; the caller's contract.
        let (Some(bytes), Some(kind), Some(reason)) =
            (unsafe { (input(in_, len), kind.as_mut(), reason.as_mut()) })
        else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        (*kind, *reason) = (0, 0);
        match Message::decode(bytes) {
            Ok(message) => {
                *kind = match message {
                    Message::Hello(_) => COMLINE_COMLINE2_HELLO,
                    Message::Answer(_) => COMLINE_COMLINE2_ANSWER,
                    Message::Verdict(_) => COMLINE_COMLINE2_VERDICT,
                };
                COMLINE_OK
            }
            Err(why) => {
                *reason = why.code();
                COMLINE_ERR_HANDSHAKE
            }
        }
    })
}

// ── free functions ─────────────────────────────────────────────────────────

/// The FNV-1a-64 hash of an encoding name (UTF-8), as the handshake carries
/// it, into `*hash`.
///
/// # Safety
/// `name` is null or readable for `len`; `hash` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_name_hash(
    name: *const u8,
    len: usize,
    hash: *mut u64,
) -> comline_status {
    guarded(|| {
        if !disjoint!(writes[(hash, 1)], reads[(name, len)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(name), Some(hash)) = (unsafe { text(name, len) }, unsafe { hash.as_mut() })
        else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        *hash = name_hash(name);
        COMLINE_OK
    })
}

/// Write a `comline@1` handshake frame (31 bytes, no length prefix), for a
/// host that runs the handshake itself. The names are UTF-8 and any name
/// is taken, `datagram@1` included; they are hashed into the frame.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` = 31),
/// `COMLINE_ERR_INVALID_ARGUMENT`.
///
/// # Safety
/// The name pointers are null or readable for their lengths; `out` is null
/// or `cap` writable bytes; `written` is null or writable.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn comline_handshake_encode(
    ir_hash: u64,
    wire_format: *const u8,
    wire_format_len: usize,
    framing: *const u8,
    framing_len: usize,
    capabilities: u32,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    guarded(|| {
        if !disjoint!(
            writes [(out, cap), (written, 1)],
            reads [(wire_format, wire_format_len), (framing, framing_len)]
        ) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(wire_format), Some(framing), Some(out), Some(written)) = (unsafe {
            (
                text(wire_format, wire_format_len),
                text(framing, framing_len),
                output(out, cap),
                written.as_mut(),
            )
        }) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let frame = HandshakeFrame::new(ir_hash, wire_format, framing, capabilities);
        let mut buf = SliceBuf::new(out);
        frame.encode(&mut buf);
        if buf.overflowed() {
            *written = COMLINE_HANDSHAKE_LEN as usize;
            return COMLINE_BUFFER_TOO_SMALL;
        }
        *written = buf.written().len();
        COMLINE_OK
    })
}

/// Check a peer's `comline@1` handshake frame against ours, for a host that
/// runs the handshake itself. `ours` is our frame as
/// `comline_handshake_encode` wrote it. `COMLINE_OK` when they agree on
/// schema, wire format and framing (capability bits may differ);
/// `COMLINE_ERR_HANDSHAKE` when they don't, or `peer` doesn't parse. As in
/// the core, bytes after the 31-byte frame are ignored (runtime#33).
///
/// `*peer_out`, unless null, gets the peer's frame whenever it parses,
/// agreeing or not, so a host can say what the peer declared.
///
/// Errors: `COMLINE_ERR_INVALID_ARGUMENT` when `ours` doesn't parse.
///
/// # Safety
/// `ours` and `peer` are null or readable for their lengths; `peer_out` is
/// null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_handshake_check(
    ours: *const u8,
    ours_len: usize,
    peer: *const u8,
    peer_len: usize,
    peer_out: *mut comline_handshake,
) -> comline_status {
    guarded(|| {
        // `peer_out` is the only memory written, so only when it's given can
        // anything overlap.
        if !peer_out.is_null()
            && !disjoint!(
                writes [(peer_out, 1)],
                reads [(ours, ours_len), (peer, peer_len)]
            )
        {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(ours), Some(peer)) = (unsafe { input(ours, ours_len) }, unsafe {
            input(peer, peer_len)
        }) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let Some(ours) = HandshakeFrame::decode(ours) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked apart from the inputs; the caller's contract.
        if let Some(out) = unsafe { peer_out.as_mut() } {
            if let Some(frame) = HandshakeFrame::decode(peer) {
                *out = comline_handshake {
                    ir_hash: frame.ir_hash,
                    wire_format: frame.wire_format,
                    framing: frame.framing,
                    capabilities: frame.capabilities,
                    _reserved: 0,
                };
            }
        }
        match ours.check_bytes(peer) {
            Ok(_) => COMLINE_OK,
            Err(error) => status_of(error),
        }
    })
}

/// 1 when this library carries the named framing or handshake (UTF-8) on a
/// connection, 0 otherwise: `datagram@2`, `datagram@3`, `jsonrpc@1`,
/// `comline@1`, `comline@2`, `none`.
/// A binding checks a schema's `@framing` here at setup, not at the first
/// frame. Wire formats are the host's codecs, so none is listed; the
/// lockstep `datagram@1` has no connection form and is 0.
///
/// # Safety
/// `name` is null or readable for `len`.
#[no_mangle]
pub unsafe extern "C" fn comline_supports(name: *const u8, len: usize) -> u8 {
    panic::catch_unwind(|| {
        // SAFETY: the caller's contract.
        let name = unsafe { text(name, len) };
        u8::from(matches!(
            name,
            Some(
                FRAMING_DATAGRAM2
                    | FRAMING_DATAGRAM3
                    | FRAMING_JSONRPC
                    | HANDSHAKE_COMLINE
                    | HANDSHAKE_COMLINE2
                    | HANDSHAKE_NONE
            )
        ))
    })
    .unwrap_or(0)
}

/// Unescape a method name as a frame spelled it (a `call_name` span with
/// `COMLINE_EVENT_NAME_ESCAPED`: the JSON string's text between its quotes)
/// into UTF-8 in `out`. Text with no escapes is copied as it is.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is the exact size needed,
/// and nothing was written), `COMLINE_ERR_INVALID_ARGUMENT` when `in` isn't
/// a valid JSON string's text.
///
/// # Safety
/// `in` is null or readable for `len`; `out` is null or `cap` writable
/// bytes; `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_method_name_unescape(
    in_: *const u8,
    len: usize,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    guarded(|| {
        if !disjoint!(writes [(out, cap), (written, 1)], reads [(in_, len)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(spelled), Some(out), Some(written)) = (
            unsafe { text(in_, len) },
            unsafe { output(out, cap) },
            unsafe { written.as_mut() },
        ) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        *written = 0;
        let Some(name) = MethodName::from_json(spelled) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let needed: usize = name.chars().map(char::len_utf8).sum();
        if needed > out.len() {
            *written = needed;
            return COMLINE_BUFFER_TOO_SMALL;
        }
        let mut at = 0;
        for c in name.chars() {
            at += c.encode_utf8(&mut out[at..]).len();
        }
        *written = at;
        COMLINE_OK
    })
}

// ── wasm32 ─────────────────────────────────────────────────────────────────

/// Linear-memory regions for a wasm32 host, which can't pass pointers into
/// its own heap: it copies input into the `in` region and reads output from
/// the `out` region. Not in the header; wasm32 only.
///
/// What a host may rely on, so it can keep a region's address and size
/// between calls and reserve again only when a call needs more (as
/// `js/comline.mjs` does):
///
/// - A region moves, grows or is freed only inside its own reserve call,
///   and only when that call asks for more bytes than it holds. A reserve
///   for no more returns the same address and changes nothing, contents
///   included. Nothing else in the module touches a region: no ABI function
///   allocates into it, frees it or writes to it except through the
///   pointers the host passes.
/// - A region is never shrunk or freed; it lives as long as the instance.
/// - Reserving one region never moves or changes the other.
/// - Memory may grow during any call (a reserve, or an allocation such as
///   `comline_connection_new`'s), which detaches the host's views of it but
///   moves nothing: every address stays valid, so the host only takes its
///   views again.
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
mod wasm {
    use core::cell::UnsafeCell;

    struct Region(UnsafeCell<Vec<u8>>);

    // SAFETY: wasm32 without the atomics feature has one thread.
    unsafe impl Sync for Region {}

    static IN: Region = Region(UnsafeCell::new(Vec::new()));
    static OUT: Region = Region(UnsafeCell::new(Vec::new()));

    /// The one place a region changes (see the module's doc): it only
    /// grows, only when `len` is more than it holds, and is never shrunk
    /// or freed.
    fn reserve(region: &Region, len: usize) -> *mut u8 {
        // SAFETY: one thread, and no reference to the region outlives a
        // call.
        let bytes = unsafe { &mut *region.0.get() };
        if bytes.len() < len {
            let more = len - bytes.len();
            if bytes.try_reserve_exact(more).is_err() {
                return core::ptr::null_mut();
            }
            bytes.resize(len, 0);
        }
        bytes.as_mut_ptr()
    }

    /// A region of at least `len` bytes for input, owned by the module;
    /// null if memory can't grow, and then the region is as it was.
    /// Reserving more than it holds may move it (and grow memory, which
    /// detaches the host's views), so a host takes the pointer again after
    /// such a reserve; a reserve for no more returns the same pointer and
    /// changes nothing.
    #[no_mangle]
    pub extern "C" fn comline_wasm_reserve_in(len: usize) -> *mut u8 {
        reserve(&IN, len)
    }

    /// As `comline_wasm_reserve_in`, for output.
    #[no_mangle]
    pub extern "C" fn comline_wasm_reserve_out(len: usize) -> *mut u8 {
        reserve(&OUT, len)
    }
}

// ── test hooks ─────────────────────────────────────────────────────────────

/// Make the next call on `conn` panic, inside its guard: the test hook for
/// the poisoning rule. Only with the `test-hooks` feature; not in the
/// header.
///
/// cbindgen:ignore
///
/// # Safety
/// `conn` is null or a live handle.
#[cfg(feature = "test-hooks")]
#[no_mangle]
pub unsafe extern "C" fn comline_test_panic_next(conn: *mut comline_connection) {
    // SAFETY: the caller's contract.
    if let Some(conn) = unsafe { conn.as_mut() } {
        conn.panic_next = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule `disjoint!` checks, as plainly as it can be said: two
    /// regions conflict when one is written, neither is empty, and each
    /// starts before the other ends. A region is `(start, len, written)`.
    fn conflict(a: (usize, usize, bool), b: (usize, usize, bool)) -> bool {
        let len = |(start, len, _): (usize, usize, bool)| if start == 0 { 0 } else { len };
        (a.2 || b.2)
            && len(a) != 0
            && len(b) != 0
            && a.0 < b.0.saturating_add(len(b))
            && b.0 < a.0.saturating_add(len(a))
    }

    /// Every three regions over a small address space (null included), and
    /// lengths up to the end of the address space, against the plain rule,
    /// with every split of the three into written and read. Not under Miri:
    /// there's no `unsafe` to check, and too many cases.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn disjoint_is_the_plain_rule() {
        let starts = [0, 1, 2, 3, 4, 5, usize::MAX - 1, usize::MAX];
        let lens = [0, 1, 2, 3, usize::MAX];
        let mut all = Vec::new();
        for &start in &starts {
            for &len in &lens {
                all.push((start as *const u8, len));
            }
        }
        let plain = |regions: [(*const u8, usize); 3], writes: usize| {
            let r = |i: usize| (regions[i].0 as usize, regions[i].1, i < writes);
            !(conflict(r(0), r(1)) || conflict(r(0), r(2)) || conflict(r(1), r(2)))
        };
        for &a in &all {
            for &b in &all {
                for &c in &all {
                    let at = [a, b, c];
                    assert_eq!(
                        disjoint!(writes [a, b, c]),
                        plain(at, 3),
                        "{at:?}, 3 written"
                    );
                    assert_eq!(
                        disjoint!(writes [a, b], reads [c]),
                        plain(at, 2),
                        "{at:?}, 2 written"
                    );
                    assert_eq!(
                        disjoint!(writes [a], reads [b, c]),
                        plain(at, 1),
                        "{at:?}, 1 written"
                    );
                }
            }
        }
    }
}
