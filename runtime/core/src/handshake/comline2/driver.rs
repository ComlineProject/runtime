use alloc::vec::Vec;

#[cfg(feature = "async")]
use crate::transport::AsyncTransport;
use crate::contract::{RuntimeError, Side, HANDSHAKE_COMLINE2};
#[cfg(feature = "async")]
use crate::handshake::AsyncHandshake;
use crate::handshake::{Facts, Handshake};
use crate::transport::Transport;
#[cfg(feature = "async")]
use futures_util::{SinkExt, StreamExt};

use super::facts::Hello;
use super::message::{Message, Refusal};
use super::negotiate::{Comline2, Negotiated};
use super::{Exchange, Params};

/// `comline@2` as a [`Handshake`]: [`Comline2`] run over a transport. After
/// it succeeds, [`negotiated`](Self::negotiated) holds what the connection
/// agreed; after it refuses, [`refusal`](Self::refusal) says why, and the
/// error is [`RuntimeError::Refused`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comline2Handshake {
    pub local: Comline2,
    negotiated: Option<Negotiated>,
    refusal: Option<Refusal>,
}

impl Comline2Handshake {
    pub fn new(params: Params, hello: Hello) -> Self {
        Self {
            local: Comline2::new(params, hello),
            negotiated: None,
            refusal: None,
        }
    }

    pub fn negotiated(&self) -> Option<&Negotiated> {
        self.negotiated.as_ref()
    }

    pub fn refusal(&self) -> Option<&Refusal> {
        self.refusal.as_ref()
    }

    fn settle(&mut self, result: Result<Negotiated, Refusal>) -> Result<(), RuntimeError> {
        match result {
            Ok(n) => {
                self.negotiated = Some(n);
                Ok(())
            }
            Err(r) => {
                self.refusal = Some(r);
                Err(r.error())
            }
        }
    }
}

/// Sends `message` when there is one.
pub(super) fn send<T: Transport>(
    transport: &mut T,
    message: &Option<Message>,
) -> Result<(), RuntimeError> {
    match message {
        Some(m) => transport.send(&m.to_bytes()),
        None => Ok(()),
    }
}

impl Handshake for Comline2Handshake {
    fn name(&self) -> &str {
        HANDSHAKE_COMLINE2
    }

    fn initiate<T: Transport>(
        &mut self,
        transport: &mut T,
        _: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        let me = Side::Initiator;
        transport.send(&self.local.hello_message().to_bytes())?;
        let mut buf = Vec::new();
        transport.recv(&mut buf)?;
        let step = match self.local.params.exchange {
            Exchange::Offer => self.local.conclude_answer(&buf),
            Exchange::Symmetric => self.local.judge_hello(me, &buf),
        };
        // Our refusal is sent best-effort: the peer may already have gone,
        // having refused first. Under `symmetric` its verdict may still be
        // there to read when ours couldn't be sent.
        let sent = send(transport, &step.reply);
        let result = match (self.local.params.exchange, step.result) {
            (Exchange::Symmetric, Ok(n)) => {
                buf.clear();
                transport
                    .recv(&mut buf)
                    .map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            (_, Ok(n)) => {
                sent?;
                Ok(n)
            }
            (_, result) => result,
        };
        self.settle(result)
    }

    fn respond<T: Transport>(
        &mut self,
        transport: &mut T,
        _: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        let me = Side::Responder;
        let mut buf = Vec::new();
        let step = match self.local.params.exchange {
            Exchange::Offer => {
                transport.recv(&mut buf)?;
                self.local.answer_offer(&buf)
            }
            Exchange::Symmetric => {
                transport.send(&self.local.hello_message().to_bytes())?;
                transport.recv(&mut buf)?;
                self.local.judge_hello(me, &buf)
            }
        };
        let sent = send(transport, &step.reply);
        let result = match step.result {
            Ok(n) => {
                buf.clear();
                transport
                    .recv(&mut buf)
                    .map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            Err(r) => Err(r),
        };
        self.settle(result)
    }
}

#[cfg(feature = "async")]
async fn send_async<T: AsyncTransport>(
    transport: &mut T,
    message: &Option<Message>,
) -> Result<(), RuntimeError> {
    match message {
        Some(m) => transport.send(m.to_bytes()).await,
        None => Ok(()),
    }
}

#[cfg(feature = "async")]
async fn recv_async<T: AsyncTransport>(transport: &mut T) -> Result<Vec<u8>, RuntimeError> {
    transport.next().await.ok_or(RuntimeError::Transport)?
}

#[cfg(feature = "async")]
impl AsyncHandshake for Comline2Handshake {
    fn name(&self) -> &str {
        HANDSHAKE_COMLINE2
    }

    async fn initiate<T: AsyncTransport>(
        &mut self,
        transport: &mut T,
        _: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        let me = Side::Initiator;
        transport
            .send(self.local.hello_message().to_bytes())
            .await?;
        let buf = recv_async(transport).await?;
        let step = match self.local.params.exchange {
            Exchange::Offer => self.local.conclude_answer(&buf),
            Exchange::Symmetric => self.local.judge_hello(me, &buf),
        };
        let sent = send_async(transport, &step.reply).await;
        let result = match (self.local.params.exchange, step.result) {
            (Exchange::Symmetric, Ok(n)) => {
                let buf = recv_async(transport)
                    .await
                    .map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            (_, Ok(n)) => {
                sent?;
                Ok(n)
            }
            (_, result) => result,
        };
        self.settle(result)
    }

    async fn respond<T: AsyncTransport>(
        &mut self,
        transport: &mut T,
        _: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        let me = Side::Responder;
        let step = match self.local.params.exchange {
            Exchange::Offer => self.local.answer_offer(&recv_async(transport).await?),
            Exchange::Symmetric => {
                transport
                    .send(self.local.hello_message().to_bytes())
                    .await?;
                self.local.judge_hello(me, &recv_async(transport).await?)
            }
        };
        let sent = send_async(transport, &step.reply).await;
        let result = match step.result {
            Ok(n) => {
                let buf = recv_async(transport)
                    .await
                    .map_err(|e| sent.err().unwrap_or(e))?;
                Comline2::finish(me, n, &buf)
            }
            Err(r) => Err(r),
        };
        self.settle(result)
    }
}
