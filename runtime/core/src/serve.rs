//! Provider-side serving: read a request frame, dispatch it, write the
//! response frame.

use alloc::vec::Vec;

use crate::contract::{
    DatagramFraming, Dispatch, Framing, Handshake, Kind, Outcome, Reply, RequestCall, RuntimeError,
    WireFormat,
};
use crate::transport::Transport;

/// Serves one protocol implementation over a [`Transport`], reusing its buffers
/// across calls (§4.6 — no per-call allocation on the frame path). Generic over
/// the [`Framing`]; defaults to the Comline datagram framing.
pub struct Server<D, W, F = DatagramFraming> {
    dispatch: D,
    format: W,
    framing: F,
    recv: Vec<u8>,
    body: Vec<u8>,
    response: Vec<u8>,
}

impl<D: Dispatch, W: WireFormat> Server<D, W, DatagramFraming> {
    pub fn new(dispatch: D, format: W) -> Self {
        Self::with_framing(dispatch, format, DatagramFraming)
    }
}

impl<D: Dispatch, W: WireFormat, F: Framing> Server<D, W, F> {
    pub fn with_framing(dispatch: D, format: W, framing: F) -> Self {
        Self {
            dispatch,
            format,
            framing,
            recv: Vec::new(),
            body: Vec::new(),
            response: Vec::new(),
        }
    }

    /// Handle one call. `Ok(true)` — a call was served; `Ok(false)` — the
    /// transport closed.
    pub fn serve_one<T: Transport>(&mut self, transport: &mut T) -> Result<bool, RuntimeError> {
        if transport.recv(&mut self.recv).is_err() {
            return Ok(false);
        }

        let req = self
            .framing
            .decode_request(&self.recv)
            .ok_or(RuntimeError::Framing)?;
        let request_id = req.request_id;

        // Whatever address the framing carried, resolve it to an ordinal.
        let idx = match req.call {
            RequestCall::Id(id) => id,
            RequestCall::Name(name) => self
                .dispatch
                .calls()
                .iter()
                .position(|c| *c == name)
                .ok_or(RuntimeError::UnknownCall)? as u16,
        };

        self.body.clear();
        let outcome = {
            let mut reply = Reply::new(&mut self.body);
            self.dispatch
                .dispatch(Kind::Id(idx), req.params, &self.format, &mut reply)?;
            reply.outcome()
        };

        self.response.clear();
        match outcome {
            // A one-way call (`_return: None`): nothing to reply.
            Outcome::None => return Ok(true),
            Outcome::Ok => self
                .framing
                .encode_response_ok(request_id, &self.body, &mut self.response),
            Outcome::Err(id) => {
                self.framing
                    .encode_response_err(request_id, id, &self.body, &mut self.response)
            }
        }
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
