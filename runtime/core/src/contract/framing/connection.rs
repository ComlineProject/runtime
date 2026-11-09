use serde::Serialize;

use crate::contract::{BufMut, Call, Envelope, Request, RuntimeError, WireFormat};

/// A protocol's number on one connection. Under `comline@1` it's the
/// protocol's position in the schema, which both ends share because their
/// IR is identical; `comline@2` will hand numbers out at the handshake.
pub type ProtocolNumber = u16;

/// How a [`Connection`](crate::connection::Connection) puts calls on the wire
/// when either end may start one. Unlike [`Framing`](crate::contract::Framing),
/// whose frames only make sense in lockstep, every frame here says whether
/// it's a request or a reply. A request names its protocol by number
/// ([`Datagram2Framing`]), or only by its call's name, which the receiver
/// looks up among the protocols it provides
/// ([`JsonRpcFraming`](crate::framing::JsonRpcFraming)).
pub trait ConnectionFraming: Default {
    /// A stable name, folded into the handshake.
    fn name(&self) -> &'static str;

    /// Frame a request to `protocol`: the call, its id, and its params
    /// (serialised with `fmt`). A `one_way` call gets no reply; a framing
    /// that can say so on the wire does.
    #[allow(clippy::too_many_arguments)]
    fn encode_request<W, P>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        request_id: u64,
        one_way: bool,
        params: &P,
        fmt: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>
    where
        W: WireFormat,
        P: Serialize + ?Sized;

    /// Frame a request to `protocol` around `params` already serialised in
    /// the connection's wire format, for a caller that encodes them itself.
    /// It writes exactly what [`encode_request`](Self::encode_request) writes
    /// when the format serialises the params to `params`. `name` is for a
    /// name-oriented framing; the others ignore it. Required, for the reason
    /// [`Framing::encode_request_body`](crate::contract::Framing::encode_request_body)
    /// gives.
    #[allow(clippy::too_many_arguments)]
    fn encode_request_body(
        &self,
        protocol: ProtocolNumber,
        call_id: u16,
        name: &str,
        request_id: u64,
        one_way: bool,
        params: &[u8],
        out: &mut dyn BufMut,
    );

    /// Frame the reply to request `request_id`, made by `protocol`.
    fn encode_reply(
        &self,
        protocol: ProtocolNumber,
        request_id: u64,
        reply: &Envelope<'_>,
        out: &mut dyn BufMut,
    );

    /// Parse a frame of either kind. Borrows `frame`.
    fn decode<'f>(&self, frame: &'f [u8]) -> Option<Frame<'f>>;

    /// Whether `frame`, which [`decode`](Self::decode) refused, is the peer
    /// saying it couldn't read a request, without saying which:
    /// [`RuntimeError::PeerRefused`] rather than a malformed frame. Only asked
    /// of a frame that failed to decode, so reading one costs nothing.
    fn is_peer_refusal(&self, _frame: &[u8]) -> bool {
        false
    }
}

/// A decoded [`ConnectionFraming`] frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame<'f> {
    /// The peer calls one of this side's protocols: `protocol`, or, when the
    /// framing names only the call, whichever of them has a call by that
    /// name.
    Request {
        protocol: Option<ProtocolNumber>,
        request: Request<'f>,
        /// The frame says the caller wants no reply (a JSON-RPC
        /// notification). A framing with no way to say so leaves it
        /// `false`; a one-way call then simply writes no reply.
        one_way: bool,
    },
    /// The peer answers a call this side made. Matched by `request_id`
    /// alone; `protocol` is there when the framing carries it.
    Reply {
        protocol: Option<ProtocolNumber>,
        request_id: u64,
        envelope: Envelope<'f>,
    },
}

/// Write `value` as an unsigned LEB128 varint, in its shortest form: how
/// `datagram@2`/`@3` and `comline@2` write a wire number.
pub fn put_varint(mut value: ProtocolNumber, out: &mut dyn BufMut) {
    while value >= 0x80 {
        out.put_u8((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.put_u8(value as u8);
}

/// An unsigned LEB128 number that fits a [`ProtocolNumber`], in its shortest
/// form, and the bytes after it.
pub fn take_varint(bytes: &[u8]) -> Option<(ProtocolNumber, &[u8])> {
    let mut value: u32 = 0;
    for (i, &byte) in bytes.iter().enumerate().take(3) {
        value |= u32::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            // A trailing zero byte would be a second spelling of a shorter
            // number.
            if i > 0 && byte == 0 {
                return None;
            }
            return Some((ProtocolNumber::try_from(value).ok()?, &bytes[i + 1..]));
        }
    }
    None
}
