//! Consumer-side calling: the mirror of [`Server`](crate::serve::Server).
//!
//! A [`Client`] owns a [`Transport`], a [`WireFormat`] and a [`Framing`], hands
//! out request ids, and reuses its send / receive buffers across calls (§4.6 —
//! no per-call allocation on the frame path). One call is: hand the params to
//! the framing (which serialises them with the format), send, block for the
//! response, hand back the [`Envelope`] borrowing the receive buffer.
//!
//! The generated `<Proto>Client` stub wraps this: it knows each function's
//! ordinal *and* name (it passes a [`Call`]), its params / result / error
//! types, and turns the returned `(Envelope, &W)` into `Result<R,
//! CallError<E>>`.

use alloc::vec::Vec;

use core::time::Duration;

use serde::Serialize;

use crate::contract::{
    Call, DatagramFraming, Envelope, Framing, Handshake, RuntimeError, WireFormat,
};
use crate::transport::Transport;

/// A calling endpoint bound to one transport. Generic over the [`Framing`];
/// defaults to the Comline datagram framing.
pub struct Client<T, W, F = DatagramFraming> {
    transport: T,
    format: W,
    framing: F,
    next_id: u64,
    request: Vec<u8>,
    response: Vec<u8>,
}

impl<T: Transport, W: WireFormat> Client<T, W, DatagramFraming> {
    /// Bind a client to a transport **without a handshake** — "misaligned
    /// mode". Use when the peer can't take part (a legacy server); a
    /// wire-format or schema mismatch then surfaces later as a decode / framing
    /// error instead of up front. [`connect`](Self::connect) is the checked
    /// path.
    pub fn new(transport: T, format: W) -> Self {
        Self::with_framing(transport, format, DatagramFraming)
    }

    /// Bind + run the connection [`Handshake`] with the datagram framing.
    pub fn connect(transport: T, format: W, local: Handshake) -> Result<Self, RuntimeError> {
        Self::connect_with_framing(transport, format, DatagramFraming, local)
    }
}

impl<T: Transport, W: WireFormat, F: Framing> Client<T, W, F> {
    pub fn with_framing(transport: T, format: W, framing: F) -> Self {
        Self {
            transport,
            format,
            framing,
            next_id: 0,
            request: Vec::new(),
            response: Vec::new(),
        }
    }

    /// [`with_framing`](Self::with_framing) + run the connection [`Handshake`]:
    /// send `local`, read the peer's, refuse (`RuntimeError::Handshake`) on a
    /// schema / wire-format / framing mismatch.
    pub fn connect_with_framing(
        transport: T,
        format: W,
        framing: F,
        local: Handshake,
    ) -> Result<Self, RuntimeError> {
        let mut client = Self::with_framing(transport, format, framing);
        client.request.clear();
        local.encode(&mut client.request);
        client.transport.send(&client.request)?;
        client.response.clear();
        client.transport.recv(&mut client.response)?;
        let peer = Handshake::decode(&client.response).ok_or(RuntimeError::Handshake)?;
        local.check(&peer)?;
        Ok(client)
    }

    /// Make `call` with `params`, block for the response, and return its
    /// [`Envelope`] (borrowing this client's receive buffer) with the format
    /// to decode it: `Ok(payload)` for the stub to read as `R`, or
    /// `Err { id, body }` to map through the generated error table.
    ///
    /// `call` is `impl Into<Call>` — a bare `u16` for datagram-only callers,
    /// or `Call::new(id, name)` from a generated stub (a name-oriented framing
    /// needs the name).
    ///
    /// Borrows `self` mutably for as long as the result is held: the previous
    /// response must be decoded (or dropped) before the next call.
    pub fn call<C, P>(&mut self, call: C, params: &P) -> Result<(Envelope<'_>, &W), RuntimeError>
    where
        C: Into<Call>,
        P: Serialize + ?Sized,
    {
        self.request_response(call.into(), params, None)
    }

    /// [`call`](Self::call), but give up after `timeout` waiting for the
    /// response — `Err(RuntimeError::Timeout)`. What a generated stub emits for
    /// a `@timeout_ms` function annotation. Honoured only by transports that
    /// override [`Transport::recv_timeout`] (the `std` ones do); others block.
    pub fn call_with_timeout<C, P>(
        &mut self,
        call: C,
        params: &P,
        timeout: Duration,
    ) -> Result<(Envelope<'_>, &W), RuntimeError>
    where
        C: Into<Call>,
        P: Serialize + ?Sized,
    {
        self.request_response(call.into(), params, Some(timeout))
    }

    fn request_response<P>(
        &mut self,
        call: Call,
        params: &P,
        timeout: Option<Duration>,
    ) -> Result<(Envelope<'_>, &W), RuntimeError>
    where
        P: Serialize + ?Sized,
    {
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.request.clear();
        self.framing
            .encode_request(call, request_id, params, &self.format, &mut self.request)?;
        self.transport.send(&self.request)?;

        self.response.clear();
        match timeout {
            None => self.transport.recv(&mut self.response)?,
            Some(d) => {
                if !self.transport.recv_timeout(&mut self.response, d)? {
                    return Err(RuntimeError::Timeout);
                }
            }
        }

        let (echoed, envelope) = self
            .framing
            .decode_response(&self.response)
            .ok_or(RuntimeError::Framing)?;
        if echoed != request_id {
            // One outstanding call at a time, so a mismatched id is a stale or
            // corrupt frame, not reordering.
            return Err(RuntimeError::Framing);
        }
        Ok((envelope, &self.format))
    }

    /// Fire a **one-way** call: frame it, send, return. No response is awaited
    /// — for `_return: None` schema functions, whose generated dispatcher
    /// writes no [`Envelope`] and whose peer [`Server`] therefore sends nothing
    /// back. `Ok(())` means the frame left the transport.
    pub fn notify<C, P>(&mut self, call: C, params: &P) -> Result<(), RuntimeError>
    where
        C: Into<Call>,
        P: Serialize + ?Sized,
    {
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.request.clear();
        self.framing.encode_request(
            call.into(),
            request_id,
            params,
            &self.format,
            &mut self.request,
        )?;
        self.transport.send(&self.request)
    }

    /// The underlying transport, e.g. to close it or read its peer address.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Consume the client, returning its transport.
    pub fn into_transport(self) -> T {
        self.transport
    }
}
