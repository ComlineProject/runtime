use comline_runtime::contract::RuntimeError;

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
pub(crate) fn status_of(error: RuntimeError) -> comline_status {
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
