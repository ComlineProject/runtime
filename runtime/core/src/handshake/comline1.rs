use alloc::vec::Vec;

#[cfg(feature = "async")]
use super::AsyncHandshake;
use super::{Facts, Handshake};
#[cfg(feature = "async")]
use crate::async_connection::AsyncTransport;
use crate::contract::{HandshakeFrame, RuntimeError, HANDSHAKE_COMLINE};
use crate::transport::Transport;
#[cfg(feature = "async")]
use futures_util::{SinkExt, StreamExt};

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
