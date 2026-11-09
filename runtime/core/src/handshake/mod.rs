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

#[cfg(feature = "async")]
use crate::async_connection::AsyncTransport;
use crate::contract::RuntimeError;
use crate::transport::Transport;
#[cfg(feature = "async")]
use core::future::Future;

mod comline1;
pub mod comline2;
mod none;

pub use comline1::ComlineHandshake;
pub use none::NoHandshake;

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
    /// The handshake's versioned name: [`HANDSHAKE_COMLINE`](crate::contract::HANDSHAKE_COMLINE),
    /// [`HANDSHAKE_NONE`](crate::contract::HANDSHAKE_NONE), or a namespaced custom one (`com.acme.hello@1`).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{HandshakeFrame, FORMAT_PACKED, FRAMING_DATAGRAM2};

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
