use serde::Serialize;

use crate::contract::{BufMut, Envelope, RuntimeError, WireFormat};

/// How a call is turned into bytes on the wire and back — the axis orthogonal
/// to [`WireFormat`] (which serialises the *parts*). The Comline datagram
/// framing ([`DatagramFraming`]) is one; JSON-RPC
/// (`comline_runtime::framing::JsonRpcFraming`) is another.
///
/// [`Client`](crate::client::Client) and [`Server`](crate::serve::Server) are
/// generic over one, chosen at setup; the pair must agree (the connection
/// [`Handshake`](crate::contract::Handshake) carries `name()`, hashed).
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

    /// Parse a request frame. The `params` slice borrows `frame` and is
    /// independently decodable with the peer's `WireFormat`.
    fn decode_request<'f>(&self, frame: &'f [u8]) -> Option<Request<'f>>;

    /// Frame an `ok` response around an already-serialised `payload`.
    fn encode_response_ok(&self, request_id: u64, payload: &[u8], out: &mut dyn BufMut);

    /// Frame an `err` response around an already-serialised error `body`,
    /// keyed by its schema-global ordinal.
    fn encode_response_err(&self, request_id: u64, id: u16, body: &[u8], out: &mut dyn BufMut);

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
    Name(&'f str),
}

// ── the Comline datagram framing ───────────────────────────────────────────

/// `no_std`, allocation-free. Request `[call_id u16 LE][request_id u64 LE]
/// [params…]`; response `[request_id u64 LE][envelope…]` where the envelope is
/// the tag-byte [`Envelope`] form.
#[derive(Debug, Default, Clone, Copy)]
pub struct DatagramFraming;

impl Framing for DatagramFraming {
    fn name(&self) -> &'static str {
        crate::contract::FRAMING_DATAGRAM
    }

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
        P: Serialize + ?Sized,
    {
        out.put_u16_le(call.id);
        out.put_u64_le(request_id);
        fmt.encode(params, out)
    }

    fn decode_request<'f>(&self, frame: &'f [u8]) -> Option<Request<'f>> {
        let (head, params) = frame.split_at_checked(10)?;
        let id = u16::from_le_bytes([head[0], head[1]]);
        let request_id = u64::from_le_bytes(head[2..10].try_into().ok()?);
        Some(Request {
            call: RequestCall::Id(id),
            request_id,
            params,
        })
    }

    fn encode_response_ok(&self, request_id: u64, payload: &[u8], out: &mut dyn BufMut) {
        out.put_u64_le(request_id);
        Envelope::encode_ok(payload, out);
    }

    fn encode_response_err(&self, request_id: u64, id: u16, body: &[u8], out: &mut dyn BufMut) {
        out.put_u64_le(request_id);
        Envelope::encode_err(id, body, out);
    }

    fn decode_response<'f>(&self, frame: &'f [u8]) -> Option<(u64, Envelope<'f>)> {
        let (head, rest) = frame.split_at_checked(8)?;
        let request_id = u64::from_le_bytes(head.try_into().ok()?);
        Some((request_id, Envelope::decode(rest)?))
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn datagram_request_round_trips() {
        // `encode_request`'s WireFormat leg is covered by the integration
        // tests; here just the header + params split.
        let mut frame = Vec::new();
        frame.extend_from_slice(&3u16.to_le_bytes());
        frame.extend_from_slice(&42u64.to_le_bytes());
        frame.extend_from_slice(b"args");

        let req = DatagramFraming.decode_request(&frame).unwrap();
        assert_eq!(req.call, RequestCall::Id(3));
        assert_eq!(req.request_id, 42);
        assert_eq!(req.params, b"args");
        assert_eq!(DatagramFraming.decode_request(&[0, 0, 0]), None);
    }

    #[test]
    fn datagram_response_round_trips() {
        let f = DatagramFraming;
        let mut ok = Vec::new();
        f.encode_response_ok(7, b"payload", &mut ok);
        assert_eq!(f.decode_response(&ok), Some((7, Envelope::Ok(b"payload"))));

        let mut err = Vec::new();
        f.encode_response_err(7, 2, b"fields", &mut err);
        assert_eq!(
            f.decode_response(&err),
            Some((7, Envelope::Err { id: 2, body: b"fields" }))
        );
    }
}
