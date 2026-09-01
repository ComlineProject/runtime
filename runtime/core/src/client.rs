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

use serde::Serialize;

use crate::contract::{Envelope, RuntimeError, WireFormat};
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
    pub fn new(transport: T, format: W) -> Self {
        Self {
            transport,
            format,
            next_id: 0,
            request: Vec::new(),
            response: Vec::new(),
        }
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
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        self.request.clear();
        wire::encode_request_header(call_id, request_id, &mut self.request);
        self.format.encode(params, &mut self.request)?;
        self.transport.send(&self.request)?;

        self.response.clear();
        self.transport.recv(&mut self.response)?;

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

    /// The underlying transport, e.g. to close it or read its peer address.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Consume the client, returning its transport.
    pub fn into_transport(self) -> T {
        self.transport
    }
}
