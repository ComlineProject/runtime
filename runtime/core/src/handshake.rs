//! The connection handshake, a declared axis beside format and framing.
//!
//! What runs when a connection opens is a [`Handshake`]: Comline's own
//! `comline@1` ([`ComlineHandshake`]), `none` ([`NoHandshake`]), or a custom
//! one for a peer that speaks its own hello. A schema picks it per protocol;
//! generated code hands it to [`Client::connect_with`](crate::client::Client::connect_with)
//! / [`Server::serve_with`](crate::serve::Server::serve_with).
//!
//! A handshake that doesn't establish schema and encoding agreement gives those
//! guarantees up, which the schema has to say (`@unverified`).

use alloc::vec::Vec;

use crate::contract::{HandshakeFrame, RuntimeError, HANDSHAKE_COMLINE, HANDSHAKE_NONE};
use crate::transport::Transport;

/// What one end knows when a connection opens, handed to its [`Handshake`]:
/// the schema fingerprint the generated code was built from, the names of the
/// format and framing it speaks, and its transport capability bits.
///
/// `#[non_exhaustive]`: more facts follow (the package version, per-protocol
/// lines) as handshakes that use them do. Build one with [`Facts::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Facts<'a> {
    /// Fingerprint of the frozen IR (a generated `IR_HASH`).
    pub ir_hash: u64,
    /// The [`WireFormat::name`](crate::contract::WireFormat::name), e.g. `json@1`.
    pub wire_format: &'a str,
    /// The [`Framing::name`](crate::contract::Framing::name), e.g. `datagram@1`.
    pub framing: &'a str,
    /// Transport capability bits (reliable / ordered / duplex / …).
    pub capabilities: u32,
}

impl<'a> Facts<'a> {
    pub fn new(ir_hash: u64, wire_format: &'a str, framing: &'a str) -> Self {
        Self {
            ir_hash,
            wire_format,
            framing,
            capabilities: 0,
        }
    }

    pub fn with_capabilities(self, capabilities: u32) -> Self {
        Self {
            capabilities,
            ..self
        }
    }
}

/// What runs on a connection before the first call.
///
/// The consumer side [`initiate`](Self::initiate)s, the provider side
/// [`respond`](Self::respond)s; for a symmetric handshake the two are the
/// same. Either refuses the connection with `Err` (by convention
/// [`RuntimeError::Handshake`]).
pub trait Handshake {
    /// The handshake's versioned name: [`HANDSHAKE_COMLINE`],
    /// [`HANDSHAKE_NONE`], or a namespaced custom one (`com.acme.hello@1`).
    fn name(&self) -> &str;

    /// Run the consumer's side over `transport`.
    fn initiate<T: Transport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError>;

    /// Run the provider's side over `transport`.
    fn respond<T: Transport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError>;
}

/// `comline@1`: each end sends its [`HandshakeFrame`] and refuses unless the
/// peer's names the same schema, format and framing. Establishes schema and
/// encoding agreement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComlineHandshake;

impl ComlineHandshake {
    fn exchange<T: Transport>(transport: &mut T, local: &Facts<'_>) -> Result<(), RuntimeError> {
        let ours = HandshakeFrame::new(
            local.ir_hash,
            local.wire_format,
            local.framing,
            local.capabilities,
        );
        let mut buf = Vec::new();
        ours.encode(&mut buf);
        transport.send(&buf)?;

        buf.clear();
        transport.recv(&mut buf)?;
        let peer = HandshakeFrame::decode(&buf).ok_or(RuntimeError::Handshake)?;
        ours.check(&peer)
    }
}

impl Handshake for ComlineHandshake {
    fn name(&self) -> &str {
        HANDSHAKE_COMLINE
    }

    fn initiate<T: Transport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        Self::exchange(transport, local)
    }

    fn respond<T: Transport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        Self::exchange(transport, local)
    }
}

/// `none`: nothing is sent or checked. Gives up schema and encoding agreement;
/// a mismatch surfaces later as a decode or framing error, if at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoHandshake;

impl Handshake for NoHandshake {
    fn name(&self) -> &str {
        HANDSHAKE_NONE
    }

    fn initiate<T: Transport>(&mut self, _: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn respond<T: Transport>(&mut self, _: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        Ok(())
    }
}
