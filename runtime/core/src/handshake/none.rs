#[cfg(feature = "async")]
use super::AsyncHandshake;
use super::{Facts, Handshake};
#[cfg(feature = "async")]
use crate::async_connection::AsyncTransport;
use crate::contract::{RuntimeError, HANDSHAKE_NONE};
use crate::transport::Transport;

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

#[cfg(feature = "async")]
impl AsyncHandshake for NoHandshake {
    fn name(&self) -> &str {
        HANDSHAKE_NONE
    }

    async fn initiate<T: AsyncTransport>(
        &mut self,
        _: &mut T,
        _: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    async fn respond<T: AsyncTransport>(
        &mut self,
        _: &mut T,
        _: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
}
