use serde::Serialize;

use crate::contract::{BufMut, Envelope, MethodName, RuntimeError, WireFormat};

mod connection;
mod datagram;
mod datagram2;
mod datagram3;
#[cfg(all(test, feature = "alloc"))]
mod tests;

pub use connection::{put_varint, take_varint, ConnectionFraming, Frame, ProtocolNumber};
pub use datagram::DatagramFraming;
pub use datagram2::Datagram2Framing;
pub use datagram3::Datagram3Framing;

/// How a call is turned into bytes on the wire and back — the axis orthogonal
/// to [`WireFormat`] (which serialises the *parts*). The Comline datagram
/// framing ([`DatagramFraming`]) is one; JSON-RPC
/// (`comline_runtime::framing::JsonRpcFraming`) is another.
///
/// [`Client`](crate::client::Client) and [`Server`](crate::serve::Server) are
/// generic over one, chosen at setup; the pair must agree (the connection
/// [`HandshakeFrame`](crate::contract::HandshakeFrame) carries `name()`, hashed).
pub trait Framing: Default {
    /// A stable name, folded into the handshake.
    fn name(&self) -> &'static str;

    /// Frame a request: the call, its id, and its params (serialised with
    /// `fmt`, or wrapped by the framing — JSON-RPC nests them).
    fn encode_request<W, P>(
        &self,
        call: Call,
        request_id: u64,
        params: &P,
        fmt: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>
    where
        W: WireFormat,
        P: Serialize + ?Sized;

    /// Frame a request around `params` already serialised in the
    /// connection's wire format, for a caller that encodes them itself: a
    /// binding in another language, or anything schema-driven. It writes
    /// exactly what [`encode_request`](Self::encode_request) writes when the
    /// format serialises the params to `params`. `name` is for a
    /// name-oriented framing; the others ignore it.
    ///
    /// Deliberately **required**, like
    /// [`encode_response_fault`](Self::encode_response_fault): a default
    /// would have to route the bytes through `encode_request` with a format
    /// that ignores its value, which only works for a framing that writes
    /// the params once and as they are. Each framing writes its own.
    fn encode_request_body(
        &self,
        call_id: u16,
        name: &str,
        request_id: u64,
        params: &[u8],
        out: &mut dyn BufMut,
    );

    /// Parse a request frame. The `params` slice borrows `frame` and is
    /// independently decodable with the peer's `WireFormat`.
    fn decode_request<'f>(&self, frame: &'f [u8]) -> Option<Request<'f>>;

    /// Whether `frame`, which [`decode_response`](Self::decode_response)
    /// refused, is the peer saying it couldn't read a request, without saying
    /// which: [`RuntimeError::PeerRefused`] rather than a malformed frame. Only
    /// asked of a frame that failed to decode, so reading one costs nothing.
    fn is_peer_refusal(&self, _frame: &[u8]) -> bool {
        false
    }

    /// Frame an `ok` response around an already-serialised `payload`.
    fn encode_response_ok(&self, request_id: u64, payload: &[u8], out: &mut dyn BufMut);

    /// Frame an `err` response around an already-serialised error `body`,
    /// keyed by its schema-global ordinal.
    fn encode_response_err(&self, request_id: u64, id: u16, body: &[u8], out: &mut dyn BufMut);

    /// Frame a `fault` response — the provider failed outside the schema
    /// ([`Envelope::Fault`]). There is no body and no ordinal.
    ///
    /// Deliberately **required**, with no default. A default would have to pick
    /// some wire shape — a reserved ordinal, say — and every framing written
    /// afterwards would silently inherit it, including ones whose wire has a
    /// perfectly good native way to say "internal error". Each framing decides
    /// for itself; the datagram framing spends a tag byte,
    /// [`JsonRpcFraming`](crate::framing::JsonRpcFraming) uses JSON-RPC's own
    /// `-32603`.
    ///
    /// Whatever it writes, [`decode_response`](Self::decode_response) must read
    /// back as [`Envelope::Fault`].
    fn encode_response_fault(&self, request_id: u64, out: &mut dyn BufMut);

    /// Parse a response frame into `(request_id, envelope)`; the envelope
    /// borrows `frame`.
    fn decode_response<'f>(&self, frame: &'f [u8]) -> Option<(u64, Envelope<'f>)>;
}

/// A call to make, carrying *both* addresses so the framing picks: the
/// append-only ordinal for [`DatagramFraming`], the name for a name-oriented
/// framing. Generated stubs emit both; `From<u16>` covers hand-written
/// datagram-only callers (`name` is then `""`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call {
    pub id: u16,
    pub name: &'static str,
}

impl Call {
    pub const fn new(id: u16, name: &'static str) -> Self {
        Self { id, name }
    }
}

impl From<u16> for Call {
    fn from(id: u16) -> Self {
        Self { id, name: "" }
    }
}

/// A decoded request. `call` is whichever address the framing put on the wire;
/// [`Server`](crate::serve::Server) resolves it to an index via
/// [`Dispatch::calls`](crate::contract::Dispatch::calls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'f> {
    pub call: RequestCall<'f>,
    pub request_id: u64,
    pub params: &'f [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestCall<'f> {
    Id(u16),
    /// The method's name, as the frame spells it.
    Name(MethodName<'f>),
}
