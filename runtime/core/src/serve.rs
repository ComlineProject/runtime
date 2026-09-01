//! Provider-side serving: read a request frame, dispatch it, write the
//! response frame.

use alloc::vec::Vec;

use crate::contract::{Dispatch, Handshake, Kind, RuntimeError, WireFormat};
use crate::transport::Transport;
use crate::wire;

/// Serves one protocol implementation over a [`Transport`], reusing its buffers
/// across calls (§4.6 — no per-call allocation on the frame path).
pub struct Server<D, W> {
    dispatch: D,
    format: W,
    recv: Vec<u8>,
    envelope: Vec<u8>,
    response: Vec<u8>,
}

impl<D: Dispatch, W: WireFormat> Server<D, W> {
    pub fn new(dispatch: D, format: W) -> Self {
        Self {
            dispatch,
            format,
            recv: Vec::new(),
            envelope: Vec::new(),
            response: Vec::new(),
        }
    }

    /// Handle one call. `Ok(true)` — a call was served; `Ok(false)` — the
    /// transport closed.
    pub fn serve_one<T: Transport>(&mut self, transport: &mut T) -> Result<bool, RuntimeError> {
        if transport.recv(&mut self.recv).is_err() {
            return Ok(false);
        }

        let (call_id, request_id, params) =
            wire::decode_request(&self.recv).ok_or(RuntimeError::Framing)?;

        self.envelope.clear();
        self.dispatch
            .dispatch(Kind::Id(call_id), params, &self.format, &mut self.envelope)?;

        // A one-way call (`_return: None`): the generated dispatcher ran the
        // handler and wrote no [`Envelope`] — there is nothing to reply.
        // Any real envelope is at least one tag byte, so "empty" is
        // unambiguous.
        if self.envelope.is_empty() {
            return Ok(true);
        }

        self.response.clear();
        wire::encode_response(request_id, &self.envelope, &mut self.response);
        transport.send(&self.response)?;
        Ok(true)
    }

    /// Serve calls until the transport closes. **No handshake** — pair with a
    /// `Client::new` peer, or an aligned setup where a mismatch can't happen.
    pub fn serve<T: Transport>(&mut self, transport: &mut T) -> Result<(), RuntimeError> {
        while self.serve_one(transport)? {}
        Ok(())
    }

    /// Run the connection [`Handshake`] against the connecting peer — send
    /// `local`, read theirs, refuse (`RuntimeError::Handshake`) on a schema /
    /// wire-format / framing mismatch — then [`serve`](Self::serve).
    pub fn serve_handshaked<T: Transport>(
        &mut self,
        transport: &mut T,
        local: Handshake,
    ) -> Result<(), RuntimeError> {
        self.response.clear();
        local.encode(&mut self.response);
        transport.send(&self.response)?;

        self.recv.clear();
        transport.recv(&mut self.recv)?;
        let peer = Handshake::decode(&self.recv).ok_or(RuntimeError::Handshake)?;
        local.check(&peer)?;

        self.serve(transport)
    }
}
