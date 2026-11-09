use crate::{COMLINE_DEFAULT_MAX_FRAME, COMLINE_MAX_FRAME_CEILING, COMLINE_PREFIX_LEN};
use comline_runtime::contract::stream::{DEFAULT_MAX_FRAME, PREFIX_LEN};
use comline_runtime::contract::{Reason, Side};

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
