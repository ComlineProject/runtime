// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline-runtime-ffi`: the C ABI over `comline-runtime`.
//!
//! It carries what is bytes and integers: the `comline@1` handshake, the
//! envelopes, the connection framings (`datagram@2`, `jsonrpc@1`), the
//! connection state (request ids, waiting calls, stale replies) and the frame
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
//!   `comline_connection_free` frees it.
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
//!   `COMLINE_ERR_POISONED` for the call that panicked. On wasm32 a panic is
//!   a trap instead, and the host discards the instance.
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

use comline_runtime::connection::{Event, State};
use comline_runtime::contract::stream::{self, Split, DEFAULT_MAX_FRAME, PREFIX_LEN};
use comline_runtime::contract::{
    name_hash, BufMut, ConnectionFraming, CountBuf, Datagram2Framing, Envelope, HandshakeFrame,
    MethodName, RequestCall, RuntimeError, SliceBuf, FRAMING_DATAGRAM2, FRAMING_JSONRPC,
    HANDSHAKE_COMLINE, HANDSHAKE_NONE,
};
use comline_runtime::framing::JsonRpcFraming;

// ── versions ───────────────────────────────────────────────────────────────

/// The ABI's major version: it changes when an existing signature, struct
/// layout or code meaning changes. 0 until the ABI is frozen at 1.0.
pub const COMLINE_ABI_MAJOR: u32 = 0;
/// The ABI's minor version: it changes when functions, codes or flag bits
/// are added. Before 1.0 a binding accepts only its own minor.
pub const COMLINE_ABI_MINOR: u32 = 1;

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

/// How to set up a connection, read once by `comline_connection_new`. The
/// name pointers are borrowed for that call only.
///
/// A connection always speaks a **connection framing**, whose frames say
/// whether they're a request or a reply, so either end can call the other:
/// `datagram@2` or `jsonrpc@1`. The lockstep `datagram@1` has no connection
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
    /// it, hashed.
    pub wire_format: *const u8,
    pub wire_format_len: usize,
    /// The framing's name: `"datagram@2"` or `"jsonrpc@1"`. UTF-8, not
    /// NUL-terminated.
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
    /// `COMLINE_HANDSHAKE_COMLINE` or `COMLINE_HANDSHAKE_NONE`.
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

/// `comline_event.flags`: `protocol` holds the protocol the request is for.
/// Set on `datagram@2` requests. A `jsonrpc@1` request names only its
/// method; the protocol is whichever one this side provides with a call by
/// that name.
pub const COMLINE_EVENT_HAS_PROTOCOL: u8 = 1 << 0;
/// `comline_event.flags`: the request is one-way and wants no reply. Set
/// whenever the frame says so (a `jsonrpc@1` notification), and never
/// inferred from a request id of 0. `datagram@2` has no way to say it on
/// the wire, so there a one-way call is told apart by the schema alone.
pub const COMLINE_EVENT_ONE_WAY: u8 = 1 << 1;
/// `comline_event.flags`: the request names its call by `call_name`
/// (`jsonrpc@1`), not by `call_id`.
pub const COMLINE_EVENT_CALL_BY_NAME: u8 = 1 << 2;
/// `comline_event.flags`: the frame has no body: JSON-RPC `params`,
/// `result` or `error.data` absent, or `null` (the framing can't tell the
/// two apart). `body` is empty, and a JSON codec reads it as `null`.
pub const COMLINE_EVENT_BODY_ABSENT: u8 = 1 << 3;
/// `comline_event.flags`: `call_name` is JSON string text with escapes in
/// it, as the frame spelled it (`app.Chat\/count`). Read it with
/// `comline_method_name_unescape`, or compare it as JSON text.
pub const COMLINE_EVENT_NAME_ESCAPED: u8 = 1 << 4;

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
    /// `BODY_ABSENT` | `NAME_ESCAPED`.
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

    // The header's numbers are literals; these hold them to the core's.
    assert!(COMLINE_DEFAULT_MAX_FRAME == DEFAULT_MAX_FRAME as u64);
    assert!(COMLINE_PREFIX_LEN as usize == PREFIX_LEN);
    assert!(COMLINE_MAX_FRAME_CEILING == u32::MAX as u64 - PREFIX_LEN as u64);
    assert!(COMLINE_DEFAULT_MAX_FRAME <= COMLINE_MAX_FRAME_CEILING);
};

// ── the connection ─────────────────────────────────────────────────────────

/// One end of a connection. Opaque: made by `comline_connection_new`, freed
/// by `comline_connection_free`.
pub struct comline_connection {
    state: State,
    framing: Framing,
    /// Our `comline@1` frame; `None` without a handshake.
    ours: Option<HandshakeFrame>,
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
    JsonRpc,
}

/// Runs `$body` with `$f` bound to the framing as its concrete type.
macro_rules! with_framing {
    ($framing:expr, |$f:ident| $body:expr) => {
        match $framing {
            Framing::Datagram2 => {
                let $f = &Datagram2Framing;
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

/// Memory one call reads or writes: its address, its length in bytes, and
/// whether the call writes it.
#[derive(Clone, Copy)]
struct Region {
    start: usize,
    len: usize,
    written: bool,
}

/// `len` bytes the call reads at `ptr`.
fn reads<T>(ptr: *const T, len: usize) -> Region {
    region(
        ptr as usize,
        len.saturating_mul(core::mem::size_of::<T>()),
        false,
    )
}

/// `len` bytes the call writes at `ptr`.
fn writes<T>(ptr: *mut T, len: usize) -> Region {
    region(
        ptr as usize,
        len.saturating_mul(core::mem::size_of::<T>()),
        true,
    )
}

/// A null pointer is no memory: the call refuses it on its own.
fn region(start: usize, len: usize, written: bool) -> Region {
    Region {
        start,
        len: if start == 0 { 0 } else { len },
        written,
    }
}

/// Whether no memory a call writes shares a byte with any other memory it
/// takes. A `&mut` that aliases another reference is undefined behaviour,
/// so this is checked on addresses alone, before any reference is made.
fn disjoint(regions: &[Region]) -> bool {
    regions.iter().enumerate().all(|(i, a)| {
        regions[i + 1..].iter().all(|b| {
            !(a.written || b.written)
                || a.len == 0
                || b.len == 0
                || a.start.saturating_add(a.len) <= b.start
                || b.start.saturating_add(b.len) <= a.start
        })
    })
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
    Some(comline_span {
        offset: u32::try_from(start).expect("spans fit u32 under the frame ceiling"),
        len: u32::try_from(part.len()).expect("spans fit u32 under the frame ceiling"),
    })
}

/// A body's span and, when the frame had none, `COMLINE_EVENT_BODY_ABSENT`.
fn body_span(base: &[u8], body: &[u8]) -> (comline_span, u8) {
    match span_in(base, body) {
        Some(span) => (span, 0),
        None => (comline_span::default(), COMLINE_EVENT_BODY_ABSENT),
    }
}

/// A method name's span as the frame spells it, and whether that spelling
/// has escapes.
///
/// [`MethodName`] lends its text only when it has no escapes
/// ([`MethodName::as_str`]); an escaped name's raw text isn't exposed. So
/// for one (rare: no Comline runtime writes a generated name with escapes)
/// the `method` member is found again in the frame, which `frame` already
/// parsed as JSON.
fn name_span(base: &[u8], frame: &[u8], name: MethodName<'_>) -> (comline_span, u8) {
    if let Some(plain) = name.as_str() {
        let span = span_in(base, plain.as_bytes()).expect("a plain name borrows the frame");
        return (span, 0);
    }
    #[derive(serde::Deserialize)]
    struct Method<'a> {
        #[serde(borrow)]
        method: &'a serde_json::value::RawValue,
    }
    let json = core::str::from_utf8(frame).expect("the frame was read as JSON");
    let parsed: Method<'_> = serde_json::from_str(json).expect("the frame was read as JSON");
    let written = parsed.method.get();
    let inner = &written[1..written.len() - 1];
    let span = span_in(base, inner.as_bytes()).expect("the name borrows the frame");
    (span, COMLINE_EVENT_NAME_ESCAPED)
}

// ── making and freeing a connection ────────────────────────────────────────

/// Make a connection from `cfg`, into `*out`. The one allocation the library
/// makes: the handle, with room for `max_waiting` waiting calls.
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
    guarded(|| {
        if out.is_null() {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        if cfg.is_null() {
            // SAFETY: checked non-null; the caller's contract.
            unsafe { out.write(ptr::null_mut()) };
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        if !disjoint(&[writes(out, 1), reads(cfg, 1)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null and apart; the caller's contract.
        let cfg = unsafe { &*cfg };
        let names = [
            reads(cfg.framing, cfg.framing_len),
            reads(cfg.wire_format, cfg.wire_format_len),
        ];
        if !disjoint(&[writes(out, 1), names[0], names[1]]) {
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
            FRAMING_JSONRPC => Framing::JsonRpc,
            _ => return COMLINE_ERR_UNKNOWN_FRAMING,
        };
        let ours = match cfg.handshake {
            COMLINE_HANDSHAKE_COMLINE => Some(HandshakeFrame::new(
                cfg.ir_hash,
                wire_format,
                framing_name,
                cfg.capabilities,
            )),
            COMLINE_HANDSHAKE_NONE => None,
            _ => return COMLINE_ERR_INVALID_ARGUMENT,
        };
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
            sent: ours.is_none(),
            received: ours.is_none(),
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

// ── the handshake on a connection ──────────────────────────────────────────

/// Write this side's handshake frame (with its length prefix in stream
/// mode), to send before anything else. Without a handshake it writes
/// nothing and returns `COMLINE_OK`.
///
/// The peer's frame is the first one fed (`COMLINE_EVENT_HANDSHAKE_OK`), and
/// may arrive before or after this call. Requests and replies need both.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` = 31, or 35 in stream
/// mode), `COMLINE_ERR_FRAME_TOO_LARGE` (`max_frame` below 31),
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
    if !disjoint(&[writes(conn, 1), writes(out, cap), writes(written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(out), Some(written)) = (output(out, cap), written.as_mut()) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *written = 0;
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
/// After that, a frame is a request, a reply to a waiting call (which then
/// waits no more), or a stale reply. A malformed frame is
/// `COMLINE_ERR_FRAMING` with the frame consumed. In stream mode, a prefix
/// over `max_frame` is `COMLINE_ERR_FRAME_TOO_LARGE` and closes the
/// connection.
///
/// **What each framing reads:**
/// - `datagram@2`: the protocol number (`COMLINE_EVENT_HAS_PROTOCOL`), the
///   `call_id` and the request id. It can't mark a call one-way.
/// - `jsonrpc@1`: the method (`COMLINE_EVENT_CALL_BY_NAME`; with
///   `COMLINE_EVENT_NAME_ESCAPED` when the frame spelled it with escapes) and
///   the `id`. A notification is `COMLINE_EVENT_ONE_WAY` with request id 0.
///   Absent `params`, `result` or `error.data` is
///   `COMLINE_EVENT_BODY_ABSENT`.
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
    let regions = [
        writes(conn, 1),
        reads(in_, in_len),
        writes(consumed, 1),
        writes(ev, 1),
    ];
    if !disjoint(&regions) {
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
                    let (span, escaped) = name_span(input, frame, name);
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
    let regions = [
        writes(conn, 1),
        reads(name, name_len),
        reads(body, body_len),
        writes(out, cap),
        writes(written, 1),
        writes(request_id, 1),
    ];
    if !disjoint(&regions) {
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
/// **What each framing reads:** `datagram@2` writes `protocol` (the one the
/// request was for) and the request id; `jsonrpc@1` ignores `protocol`,
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
    let regions = [
        writes(conn, 1),
        reads(body, body_len),
        writes(out, cap),
        writes(written, 1),
    ];
    if !disjoint(&regions) {
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
    if !disjoint(&[writes(conn, 1), writes(waiting, 1)]) {
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
        if !disjoint(&[reads(name, len), writes(hash, 1)]) {
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
        let regions = [
            reads(wire_format, wire_format_len),
            reads(framing, framing_len),
            writes(out, cap),
            writes(written, 1),
        ];
        if !disjoint(&regions) {
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
        let regions = [
            reads(ours, ours_len),
            reads(peer, peer_len),
            writes(peer_out, 1),
        ];
        if !disjoint(&regions) {
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
/// connection, 0 otherwise: `datagram@2`, `jsonrpc@1`, `comline@1`, `none`.
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
            Some(FRAMING_DATAGRAM2 | FRAMING_JSONRPC | HANDSHAKE_COMLINE | HANDSHAKE_NONE)
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
        if !disjoint(&[reads(in_, len), writes(out, cap), writes(written, 1)]) {
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
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
mod wasm {
    use core::cell::UnsafeCell;

    struct Region(UnsafeCell<Vec<u8>>);

    // SAFETY: wasm32 without the atomics feature has one thread.
    unsafe impl Sync for Region {}

    static IN: Region = Region(UnsafeCell::new(Vec::new()));
    static OUT: Region = Region(UnsafeCell::new(Vec::new()));

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
    /// null if memory can't grow. Reserving more may move it (and grow
    /// memory, which detaches the host's views), so a host reserves up
    /// front and takes the pointer again after each reserve.
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
