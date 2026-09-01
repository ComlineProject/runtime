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
    /// The connection handshake disagreed on schema hash, wire format, or
    /// framing — or none arrived. Never raised when the handshake is skipped
    /// (`Client::new` / `Server::serve` — "misaligned mode").
    Handshake,
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
            RuntimeError::Handshake => f.write_str("connection handshake mismatch"),
        }
    }
}

impl core::error::Error for RuntimeError {}
