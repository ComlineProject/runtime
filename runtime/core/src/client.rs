//! Consumer-side calling: the mirror of [`Server`](crate::serve::Server).
//!
//! A [`Client`] owns a [`Transport`] and a [`WireFormat`], hands out request
//! ids, and reuses its send / receive buffers across calls (§4.6 — no per-call
//! allocation on the frame path). One call is: encode the params straight into
//! the frame, send, block for the response, hand back the [`Envelope`]
//! borrowing the receive buffer.
//!
//! The generated `<Proto>Client` stub wraps this: it knows each function's
//! ordinal and its params / result / error types, so it calls [`Client::call`]
//! with a `u16` and turns the returned `(Envelope, &W)` into `Result<R,
//! CallError<E>>` — `Ok` payload decoded as `R`, an `Err` ordinal mapped through
//! its generated error table. This type stays protocol-agnostic; it does no
//! decoding, which is why `call` also hands back the format.

use alloc::vec::Vec;

use core::time::Duration;

use serde::Serialize;

use crate::contract::{Envelope, Handshake, RuntimeError, WireFormat};
use crate::transport::Transport;
use crate::wire;

/// A calling endpoint bound to one transport.
pub struct Client<T, W> {
    transport: T,
    format: W,
    next_id: u64,
    request: Vec<u8>,
    response: Vec<u8>,
}

impl<T: Transport, W: WireFormat> Client<T, W> {
    /// Bind a client to a transport **without a handshake** — "misaligned
    /// mode". Use when the peer can't take part (a legacy server); a
    /// wire-format or schema mismatch then surfaces later as a decode / framing
    /// error instead of up front. [`connect`](Self::connect) is the checked
    /// path.
    pub fn new(transport: T, format: W) -> Self {
        Self {
            transport,
            format,
            next_id: 0,
            request: Vec::new(),
            response: Vec::new(),
        }
    }

    /// Bind a client and run the connection [`Handshake`]: send `local`, read
    /// the peer's, and refuse (`RuntimeError::Handshake`) if they disagree on
    /// schema hash, wire format, or framing.
    pub fn connect(transport: T, format: W, local: Handshake) -> Result<Self, RuntimeError> {
        let mut client = Self::new(transport, format);
        client.exchange_handshake(local)?;
        Ok(client)
    }

    fn exchange_handshake(&mut self, local: Handshake) -> Result<(), RuntimeError> {
        self.request.clear();
        local.encode(&mut self.request);
        self.transport.send(&self.request)?;

        self.response.clear();
        self.transport.recv(&mut self.response)?;
        let peer = Handshake::decode(&self.response).ok_or(RuntimeError::Handshake)?;
        local.check(&peer)
    }

    /// Make call `call_id` with `params`, block for the response, and return
    /// its [`Envelope`] (borrowing this client's receive buffer) together with
    /// the format to decode it: `Ok(payload)` for the stub to read as `R`, or
    /// `Err { id, body }` to map through the generated error table.
    ///
    /// The format rides along because both views come out of the same
    /// `&mut self` borrow — the stub can't reach back into the client for it
    /// while holding the envelope.
    ///
    /// Borrows `self` mutably for as long as the result is held: the previous
    /// response must be decoded (or dropped) before the next call. A pipelined
    /// / multiplexed client is a later, additive layer.
    pub fn call<P>(&mut self, call_id: u16, params: &P) -> Result<(Envelope<'_>, &W), RuntimeError>
    where
        P: Serialize + ?Sized,
    {
        self.request_response(call_id, params, None)
    }

    /// [`call`](Self::call), but give up after `timeout` waiting for the
    /// response — `Err(RuntimeError::Timeout)`. What a generated stub emits for
    /// a `@timeout_ms` function annotation. Honoured only by transports that
    /// override [`Transport::recv_timeout`] (the `std` ones do); others block.
    pub fn call_with_timeout<P>(
        &mut self,
        call_id: u16,
        params: &P,
        timeout: Duration,
    ) -> Result<(Envelope<'_>, &W), RuntimeError>
    where
        P: Serialize + ?Sized,
    {
        self.request_response(call_id, params, Some(timeout))
    }

    fn request_response<P>(
        &mut self,
        call_id: u16,
        params: &P,
        timeout: Option<Duration>,
    ) -> Result<(Envelope<'_>, &W), RuntimeError>
    where
        P: Serialize + ?Sized,
    {
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.request.clear();
        wire::encode_request_header(call_id, request_id, &mut self.request);
        self.format.encode(params, &mut self.request)?;
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

        let (echoed, envelope) =
            wire::decode_response(&self.response).ok_or(RuntimeError::Framing)?;
        if echoed != request_id {
            // One outstanding call at a time, so a mismatched id is a stale or
            // corrupt frame, not reordering.
            return Err(RuntimeError::Framing);
        }
        let envelope = Envelope::decode(envelope).ok_or(RuntimeError::Framing)?;
        Ok((envelope, &self.format))
    }

    /// Fire a **one-way** call: frame `call_id` + `params`, send, return. No
    /// response is awaited — for `_return: None` schema functions, whose
    /// generated dispatcher writes no [`Envelope`] and whose peer [`Server`]
    /// therefore sends nothing back. `Ok(())` means the frame left the
    /// transport, never a remote outcome.
    pub fn notify<P>(&mut self, call_id: u16, params: &P) -> Result<(), RuntimeError>
    where
        P: Serialize + ?Sized,
    {
        // Keep request ids monotonic across mixed call / notify use, even
        // though nothing reads this one back.
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.request.clear();
        wire::encode_request_header(call_id, request_id, &mut self.request);
        self.format.encode(params, &mut self.request)?;
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
