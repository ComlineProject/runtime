//! The connection handshake, a declared axis beside format and framing.
//!
//! What runs when a connection opens is a [`Handshake`]: Comline's own
//! `comline@1` ([`ComlineHandshake`]), `none` ([`NoHandshake`]), or a custom
//! one for a peer that speaks its own hello. A schema picks it per protocol;
//! generated code hands it to [`Client::connect_with`](crate::client::Client::connect_with)
//! / [`Server::serve_with`](crate::serve::Server::serve_with).
//!
//! An [`AsyncConnection`](crate::async_connection::AsyncConnection) (the
//! `async` feature) runs an [`AsyncHandshake`] instead, over its
//! [`AsyncTransport`], through
//! [`AsyncConnection::connect_with`](crate::async_connection::AsyncConnection::connect_with)
//! / [`accept_with`](crate::async_connection::AsyncConnection::accept_with).
//! [`ComlineHandshake`] and [`NoHandshake`] implement both traits and send
//! the same bytes either way. A custom handshake implements whichever its
//! connections need, or both.
//!
//! A handshake that doesn't establish schema and encoding agreement gives those
//! guarantees up, which the schema has to say (`@unverified`).

use alloc::vec::Vec;

use crate::contract::{HandshakeFrame, RuntimeError, HANDSHAKE_COMLINE, HANDSHAKE_NONE};
use crate::transport::Transport;
#[cfg(feature = "async")]
use crate::async_connection::AsyncTransport;
#[cfg(feature = "async")]
use core::future::Future;
#[cfg(feature = "async")]
use futures_util::{SinkExt, StreamExt};

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

impl HandshakeFrame {
    /// The frame `local` declares: its schema fingerprint, the hashes of its
    /// format and framing names, and its capability bits.
    pub fn from_facts(local: &Facts<'_>) -> Self {
        Self::new(
            local.ir_hash,
            local.wire_format,
            local.framing,
            local.capabilities,
        )
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
    /// Our frame, and its bytes to send. Both the blocking and the async
    /// exchange start here, so they send the same bytes.
    fn ours(local: &Facts<'_>) -> (HandshakeFrame, Vec<u8>) {
        let ours = HandshakeFrame::from_facts(local);
        let mut buf = Vec::new();
        ours.encode(&mut buf);
        (ours, buf)
    }

    fn exchange<T: Transport>(transport: &mut T, local: &Facts<'_>) -> Result<(), RuntimeError> {
        let (ours, mut buf) = Self::ours(local);
        transport.send(&buf)?;

        buf.clear();
        transport.recv(&mut buf)?;
        ours.check_bytes(&buf).map(|_peer| ())
    }

    #[cfg(feature = "async")]
    async fn exchange_async<T: AsyncTransport>(
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        let (ours, buf) = Self::ours(local);
        transport.send(buf).await?;

        let theirs = transport.next().await.ok_or(RuntimeError::Transport)??;
        ours.check_bytes(&theirs).map(|_peer| ())
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

// ── async ──────────────────────────────────────────────────────────────────

/// What runs on an [`AsyncConnection`](crate::async_connection::AsyncConnection)
/// before the first call: the async counterpart of [`Handshake`], over an
/// [`AsyncTransport`]. Needs nothing from an executor but polling.
///
/// The consumer side [`initiate`](Self::initiate)s, the provider side
/// [`respond`](Self::respond)s; either refuses the connection with `Err` (by
/// convention [`RuntimeError::Handshake`]). Implement it with `async fn`s:
///
/// ```ignore
/// impl AsyncHandshake for Hello {
///     fn name(&self) -> &str { "com.acme.hello@1" }
///
///     async fn initiate<T: AsyncTransport>(&mut self, transport: &mut T, local: &Facts<'_>)
///         -> Result<(), RuntimeError>
///     {
///         transport.send(b"HELLO".to_vec()).await?;
///         // ...
///         Ok(())
///     }
///     // respond likewise
/// }
/// ```
///
/// The futures are `Send`, so a connection opened with one can be dialled
/// from a spawned task.
#[cfg(feature = "async")]
pub trait AsyncHandshake {
    /// The handshake's versioned name, as [`Handshake::name`].
    fn name(&self) -> &str;

    /// Run the consumer's side over `transport`.
    fn initiate<T: AsyncTransport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send;

    /// Run the provider's side over `transport`.
    fn respond<T: AsyncTransport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send;
}

#[cfg(feature = "async")]
impl AsyncHandshake for ComlineHandshake {
    fn name(&self) -> &str {
        HANDSHAKE_COMLINE
    }

    async fn initiate<T: AsyncTransport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        Self::exchange_async(transport, local).await
    }

    async fn respond<T: AsyncTransport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        Self::exchange_async(transport, local).await
    }
}

#[cfg(feature = "async")]
impl AsyncHandshake for NoHandshake {
    fn name(&self) -> &str {
        HANDSHAKE_NONE
    }

    async fn initiate<T: AsyncTransport>(&mut self, _: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        Ok(())
    }

    async fn respond<T: AsyncTransport>(&mut self, _: &mut T, _: &Facts<'_>) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{FORMAT_PACKED, FRAMING_DATAGRAM2};

    #[test]
    fn from_facts_is_new_with_the_facts() {
        let facts = Facts::new(0x0102_0304_0506_0708, FORMAT_PACKED, FRAMING_DATAGRAM2)
            .with_capabilities(0b101);
        assert_eq!(
            HandshakeFrame::from_facts(&facts),
            HandshakeFrame::new(
                0x0102_0304_0506_0708,
                FORMAT_PACKED,
                FRAMING_DATAGRAM2,
                0b101
            )
        );
    }
}
