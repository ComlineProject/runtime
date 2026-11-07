use core::fmt;

/// Infrastructure failure on a call — everything that is *not* a
/// schema-declared error. Schema errors travel in the [`Envelope`] and are
/// reconstructed by generated code; a mismatch, a dead transport, or an
/// ordinal this side does not know lands here.
///
/// Deliberately lifetime-free and `'static`: it can be stored and logged
/// without borrowing the receive buffer.
///
/// [`Envelope`]: crate::contract::Envelope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeError {
    /// The transport failed to send or receive.
    Transport,
    /// A payload could not be encoded or decoded against its type.
    Serialization,
    /// The call frame was malformed.
    Framing,
    /// No response arrived within the call's window.
    Timeout,
    /// The peer addressed a call this side does not have (`Kind` out of range).
    UnknownCall,
    /// The peer raised a schema error beyond what this side's generated code
    /// knows — a newer schema. `id` is the schema-global error ordinal; the
    /// still-encoded body is not retained.
    Remote { id: u16 },
    /// The peer's provider failed in a way its schema does not describe — it
    /// returned [`Fault::Internal`](crate::contract::Fault::Internal), or its
    /// dispatcher could not complete the call. No detail crosses the wire; it
    /// stays on the provider's side to be logged.
    Provider,
    /// The connection handshake refused the peer: under `comline@1`, the two
    /// disagreed on schema hash, wire format or framing, or no frame arrived.
    /// Never raised when there is none (`none`, `Client::new_unverified` /
    /// `Server::serve_unverified`).
    Handshake,
    /// A frame larger than the limit: the transport's, by default
    /// [`DEFAULT_MAX_FRAME`](crate::contract::stream::DEFAULT_MAX_FRAME).
    ///
    /// - **On send** it is recoverable: nothing was written, and the
    ///   connection keeps working.
    /// - **On receive** the stream is out of sync past the oversized frame,
    ///   so the transport closes, and later calls return
    ///   [`Transport`](Self::Transport).
    ///
    /// A blocking `Client::call` sends and then receives, so this error
    /// alone doesn't say which side it came from. The next call returning
    /// [`Transport`](Self::Transport) is how a caller knows the connection
    /// closed.
    FrameTooLarge,
    /// The call wasn't made: this side already waits on as many calls as its
    /// [`State`](crate::connection::State) has room for. Nothing was written
    /// and no request id was spent.
    Busy,
    /// The frame didn't fit the fixed buffer it was written into
    /// ([`BufMut::overflowed`](crate::contract::BufMut::overflowed)). Nothing
    /// was committed; the caller can retry with more room.
    BufferTooSmall,
    /// A value over its type's bound (core#98): a string of more
    /// characters, an array of more items, or a type that contains itself
    /// nested deeper than its type allows. `measure` says which.
    ///
    /// The maximum itself is in the type, which the caller knows; the
    /// checks in [`bounds`](crate::bounds) return it too
    /// ([`BoundExceeded`](crate::bounds::BoundExceeded)). Leaving it out
    /// here keeps this error, and `packed@1`'s inside it, a few bytes:
    /// carrying it slowed `packed@1` decoding by up to 15%.
    ///
    /// - **On decode** it's malformed input, refused like any other, before
    ///   the value is allocated.
    /// - **On encode** nothing was written: the value isn't one its type
    ///   allows, and a conforming peer would refuse it.
    BoundExceeded { measure: crate::bounds::Measure },
    /// The `comline@2` handshake refused the connection: `side` refused it,
    /// for `reason`. Raised on both ends, the refusing side's own included.
    Refused {
        reason: crate::contract::Reason,
        side: crate::contract::Side,
    },
    /// The call wasn't made: the `comline@2` handshake found that the peer
    /// can't take it. Either the peer lacks the function (it's older, and
    /// `newer_consumer = degrade` let the connection through) or the
    /// protocol was refused on its own (`scope = protocol`). Nothing was
    /// written. The caller knows which call it made, so the error doesn't
    /// repeat it: that keeps this type 4 bytes, which the decode paths
    /// need.
    Unavailable,
    /// The peer says it couldn't read a request of this side's, and not which:
    /// a JSON-RPC error with a `null` id, as the specification has for a
    /// request it can't parse. Nothing can say which call that fails, and a
    /// peer that can't read one frame usually can't read any, so it ends the
    /// connection as a malformed frame does.
    PeerRefused,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::Transport => f.write_str("transport failure"),
            RuntimeError::Serialization => f.write_str("serialization failure"),
            RuntimeError::Framing => f.write_str("malformed call frame"),
            RuntimeError::Timeout => f.write_str("call timed out"),
            RuntimeError::UnknownCall => f.write_str("unknown call"),
            RuntimeError::Remote { id } => write!(f, "unrecognised remote error (ordinal {id})"),
            RuntimeError::Provider => f.write_str("provider failure"),
            RuntimeError::Handshake => f.write_str("connection handshake mismatch"),
            RuntimeError::FrameTooLarge => f.write_str("frame larger than the limit"),
            RuntimeError::Busy => f.write_str("too many calls waiting"),
            RuntimeError::BufferTooSmall => f.write_str("frame larger than its buffer"),
            RuntimeError::Refused { reason, side } => {
                write!(f, "the {} refused the connection: {reason}", side.name())
            }
            RuntimeError::Unavailable => f.write_str(
                "the peer can't take this call: it lacks the function, or refused its protocol",
            ),
            RuntimeError::BoundExceeded { measure } => {
                write!(f, "more {} than its type allows", measure.noun())
            }
            RuntimeError::PeerRefused => f.write_str("the peer could not read a request"),
        }
    }
}

impl core::error::Error for RuntimeError {}
