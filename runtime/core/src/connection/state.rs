use alloc::vec::Vec;

use serde::Serialize;

use crate::contract::{
    BufMut, Call, ConnectionFraming, Envelope, Frame, ProtocolNumber, Request, RuntimeError,
    WireFormat,
};

/// What a received frame means for this side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'f> {
    /// The peer calls `protocol` on this side (or, when the framing names
    /// only the call, whichever protocol has it); answer with
    /// [`State::encode_reply`] unless the call is one-way.
    Request {
        protocol: Option<ProtocolNumber>,
        request: Request<'f>,
        /// The frame asked for no reply: send none, whatever the handler
        /// returns.
        one_way: bool,
    },
    /// The reply to the innermost call this side is waiting on.
    Reply {
        request_id: u64,
        envelope: Envelope<'f>,
    },
    /// A reply to a call nobody waits on any more (it timed out). Drop it.
    Stale,
}

/// The calls one side of a connection has made and is waiting on, and the
/// request ids it hands out. No I/O: bytes in, [`Event`]s out.
///
/// For the blocking driver (`State::default()`) calls nest, so the waiting
/// calls form a stack, and a reply has to answer the innermost one: the peer
/// can't answer an outer call while it is still waiting on its own nested
/// call to this side. The async driver has several calls in flight at once,
/// answered in any order ([`unordered`](Self::unordered)).
///
/// Either way it makes room for waiting calls as they come, unless it was
/// given a fixed room ([`with_capacity`](Self::with_capacity)).
#[derive(Debug)]
pub struct State {
    next_id: u64,
    waiting: Vec<u64>,
    /// How many calls may wait at once; `usize::MAX` grows as needed.
    capacity: usize,
    unordered: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next_id: 0,
            waiting: Vec::new(),
            capacity: usize::MAX,
            unordered: false,
        }
    }
}

impl State {
    /// A state whose waiting calls may be answered in any order.
    pub fn unordered() -> Self {
        Self {
            unordered: true,
            ..Self::default()
        }
    }

    /// A state with room for `capacity` waiting calls, reserved now, for a
    /// caller that mustn't allocate once it's set up (a binding through the
    /// C ABI). A call that would wait past that is [`RuntimeError::Busy`].
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            waiting: Vec::with_capacity(capacity),
            capacity,
            ..Self::default()
        }
    }

    /// [`with_capacity`](Self::with_capacity), answered in any order.
    pub fn unordered_with_capacity(capacity: usize) -> Self {
        Self {
            unordered: true,
            ..Self::with_capacity(capacity)
        }
    }

    /// Whether a call that expects a reply would find no room to wait.
    fn full(&self, expects_reply: bool) -> bool {
        expects_reply && self.waiting.len() >= self.capacity
    }

    /// Frame a call into `out` and return its request id. A call that
    /// `expects_reply` is waited on until [`receive`](Self::receive) sees its
    /// reply or [`abandon`](Self::abandon) gives up on it. With no room for it
    /// to wait it's [`RuntimeError::Busy`], and nothing is spent; a failed
    /// encode spends its id.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_request<F, W, P>(
        &mut self,
        framing: &F,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        format: &W,
        expects_reply: bool,
        out: &mut Vec<u8>,
    ) -> Result<u64, RuntimeError>
    where
        F: ConnectionFraming,
        W: WireFormat,
        P: Serialize + ?Sized,
    {
        if self.full(expects_reply) {
            return Err(RuntimeError::Busy);
        }
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        out.clear();
        framing.encode_request(
            protocol,
            call,
            request_id,
            !expects_reply,
            params,
            format,
            out,
        )?;
        if expects_reply {
            self.waiting.push(request_id);
        }
        Ok(request_id)
    }

    /// Frame a call around `params` already serialised in the connection's
    /// wire format ([`ConnectionFraming::encode_request_body`]), appending to
    /// `out`, and return its request id.
    ///
    /// Nothing is committed unless the whole frame was written: when `out`
    /// dropped a write ([`BufMut::overflowed`]) it's
    /// [`RuntimeError::BufferTooSmall`], no id is spent and nothing waits, so
    /// the caller can retry with more room. With no room for the call to wait
    /// it's [`RuntimeError::Busy`], before anything is written.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_request_body<F: ConnectionFraming>(
        &mut self,
        framing: &F,
        protocol: ProtocolNumber,
        call_id: u16,
        name: &str,
        params: &[u8],
        expects_reply: bool,
        out: &mut dyn BufMut,
    ) -> Result<u64, RuntimeError> {
        if self.full(expects_reply) {
            return Err(RuntimeError::Busy);
        }
        let request_id = self.next_id;
        framing.encode_request_body(
            protocol,
            call_id,
            name,
            request_id,
            !expects_reply,
            params,
            out,
        );
        if out.overflowed() {
            return Err(RuntimeError::BufferTooSmall);
        }
        self.next_id = request_id.wrapping_add(1);
        if expects_reply {
            self.waiting.push(request_id);
        }
        Ok(request_id)
    }

    /// Frame the reply to the peer's request `request_id` into `out`.
    pub fn encode_reply<F: ConnectionFraming>(
        &self,
        framing: &F,
        protocol: ProtocolNumber,
        request_id: u64,
        reply: &Envelope<'_>,
        out: &mut Vec<u8>,
    ) {
        out.clear();
        framing.encode_reply(protocol, request_id, reply, out);
    }

    /// What `frame` means. A malformed frame, or (unless
    /// [`unordered`](Self::unordered)) a reply that skips the innermost
    /// waiting call, is [`RuntimeError::Framing`]; the peer saying it couldn't
    /// read a request is [`RuntimeError::PeerRefused`].
    pub fn receive<'f, F: ConnectionFraming>(
        &mut self,
        framing: &F,
        frame: &'f [u8],
    ) -> Result<Event<'f>, RuntimeError> {
        let decoded = framing.decode(frame).ok_or_else(|| {
            if framing.is_peer_refusal(frame) {
                RuntimeError::PeerRefused
            } else {
                RuntimeError::Framing
            }
        })?;
        match decoded {
            Frame::Request {
                protocol,
                request,
                one_way,
            } => Ok(Event::Request {
                protocol,
                request,
                one_way,
            }),
            Frame::Reply {
                request_id,
                envelope,
                ..
            } => {
                if self.unordered {
                    return Ok(match self.waiting.iter().position(|&id| id == request_id) {
                        Some(at) => {
                            self.waiting.swap_remove(at);
                            Event::Reply {
                                request_id,
                                envelope,
                            }
                        }
                        None => Event::Stale,
                    });
                }
                if self.waiting.last() == Some(&request_id) {
                    self.waiting.pop();
                    Ok(Event::Reply {
                        request_id,
                        envelope,
                    })
                } else if self.waiting.contains(&request_id) {
                    Err(RuntimeError::Framing)
                } else {
                    Ok(Event::Stale)
                }
            }
        }
    }

    /// Take back the call [`encode_request`](Self::encode_request) just
    /// framed as `request_id`, before it was sent: it no longer waits, and,
    /// if no call was framed since, its request id is handed out again.
    /// Call it only before the frame is sent: on a sent call it would hand
    /// that request id out a second time.
    pub(crate) fn retract(&mut self, request_id: u64) {
        self.abandon(request_id);
        if self.next_id == request_id.wrapping_add(1) {
            self.next_id = request_id;
        }
    }

    /// Stop waiting on `request_id` (it timed out); a late reply to it is
    /// then [`Event::Stale`].
    pub fn abandon(&mut self, request_id: u64) {
        self.waiting.retain(|&id| id != request_id);
    }

    /// Whether any call is waiting on a reply.
    pub fn is_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }
}
