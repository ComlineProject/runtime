use crate::{
    comline_event, comline_status, COMLINE_BUFFER_TOO_SMALL, COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_REFUSED, COMLINE_EVENT_HANDSHAKE_CONTINUE, COMLINE_EVENT_HANDSHAKE_OK,
    COMLINE_EVENT_SEND, COMLINE_OK,
};
use comline_runtime::connection::State;
use comline_runtime::contract::stream::{self, PREFIX_LEN};
use comline_runtime::contract::{BufMut, CountBuf, HandshakeFrame, Side, SliceBuf};
use comline_runtime::handshake::comline2::{Comline2, Exchange, Negotiated, Refusal, Step};

/// One end of a connection. Opaque: made by `comline_connection_new`, freed
/// by `comline_connection_free`.
pub struct comline_connection {
    pub(crate) state: State,
    pub(crate) framing: Framing,
    /// Our `comline@1` frame; `None` without one.
    pub(crate) ours: Option<HandshakeFrame>,
    /// The `comline@2` handshake and what it came to; `None` without one.
    pub(crate) comline2: Option<Box<Handshake2>>,
    /// Our handshake frame was written (always, without one).
    pub(crate) sent: bool,
    /// The peer's handshake frame arrived and agreed (always, without one).
    pub(crate) received: bool,
    /// Refused: a bad handshake, or an oversized length prefix.
    pub(crate) closed: bool,
    /// An earlier call panicked.
    pub(crate) poisoned: bool,
    pub(crate) stream: bool,
    pub(crate) max_frame: usize,
    /// The next call panics: the test hook for the poisoning rule.
    #[cfg(feature = "test-hooks")]
    pub(crate) panic_next: bool,
}

/// The connection framings, dispatched here so no generic crosses.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Framing {
    Datagram2,
    Datagram3,
    JsonRpc,
}

/// What a `comline@2` connection reads next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Awaiting {
    Hello,
    Answer,
    Verdict,
    Nothing,
}

/// `comline@2` on a connection: [`Comline2`]'s sans-IO steps, in the order
/// [`comline_runtime::handshake::comline2::Comline2Handshake`] runs them over a transport.
pub(crate) struct Handshake2 {
    pub(crate) local: Comline2,
    pub(crate) side: Side,
    /// This end still has to send its hello.
    pub(crate) hello_due: bool,
    /// The next message to send, once the hello is out.
    pub(crate) pending: Option<Vec<u8>>,
    pub(crate) awaiting: Awaiting,
    /// What it has concluded. The peer's verdict can still refuse a protocol,
    /// or the connection, until `awaiting` is `Nothing`.
    pub(crate) negotiated: Option<Negotiated>,
    pub(crate) refusal: Option<Refusal>,
}

impl Handshake2 {
    pub(crate) fn new(local: Comline2, side: Side) -> Self {
        let offer = local.params.exchange == Exchange::Offer;
        let initiates = side == Side::Initiator;
        Handshake2 {
            local,
            side,
            hello_due: !offer || initiates,
            pending: None,
            awaiting: if offer && initiates {
                Awaiting::Answer
            } else {
                Awaiting::Hello
            },
            negotiated: None,
            refusal: None,
        }
    }

    /// Whether there's a message to send.
    pub(crate) fn has_more(&self) -> bool {
        self.hello_due || self.pending.is_some()
    }

    /// The message [`Self::has_more`] speaks of, encoded.
    pub(crate) fn next_message(&self) -> Option<Vec<u8>> {
        if self.hello_due {
            Some(self.local.hello_message().to_bytes())
        } else {
            self.pending.clone()
        }
    }

    /// The message [`Self::next_message`] gave was written.
    pub(crate) fn message_sent(&mut self) {
        if self.hello_due {
            self.hello_due = false;
        } else {
            self.pending = None;
        }
    }

    /// Take in the peer's message. Its status; a refusal is held in `refusal`.
    pub(crate) fn take(&mut self, frame: &[u8], ev: &mut comline_event) -> comline_status {
        let step = match self.awaiting {
            Awaiting::Hello => match self.local.params.exchange {
                Exchange::Offer => self.local.answer_offer(frame),
                Exchange::Symmetric => self.local.judge_hello(self.side, frame),
            },
            Awaiting::Answer => self.local.conclude_answer(frame),
            Awaiting::Verdict => {
                let negotiated = self
                    .negotiated
                    .take()
                    .expect("a verdict is awaited after a conclusion");
                Step {
                    reply: None,
                    result: Comline2::finish(self.side, negotiated, frame),
                }
            }
            Awaiting::Nothing => unreachable!("a finished handshake reads no more messages"),
        };
        self.awaiting = match self.awaiting {
            Awaiting::Hello => Awaiting::Verdict,
            _ => Awaiting::Nothing,
        };
        if let Some(reply) = step.reply {
            self.pending = Some(reply.to_bytes());
        }
        let status = match step.result {
            Ok(negotiated) => {
                self.negotiated = Some(negotiated);
                ev.kind = if self.awaiting == Awaiting::Nothing {
                    COMLINE_EVENT_HANDSHAKE_OK
                } else {
                    COMLINE_EVENT_HANDSHAKE_CONTINUE
                };
                COMLINE_OK
            }
            Err(refusal) => {
                // A refusal ends it: no hello after that.
                self.hello_due = false;
                self.awaiting = Awaiting::Nothing;
                self.refusal = Some(refusal);
                COMLINE_ERR_REFUSED
            }
        };
        if self.has_more() {
            ev.flags |= COMLINE_EVENT_SEND;
        }
        status
    }
}

/// Runs `$body` with `$f` bound to the framing as its concrete type.
macro_rules! with_framing {
    ($framing:expr, |$f:ident| $body:expr) => {
        match $framing {
            $crate::connection::Framing::Datagram2 => {
                let $f = &Datagram2Framing;
                $body
            }
            $crate::connection::Framing::Datagram3 => {
                let $f = &Datagram3Framing;
                $body
            }
            $crate::connection::Framing::JsonRpc => {
                let $f = &JsonRpcFraming;
                $body
            }
        }
    };
}

impl comline_connection {
    pub(crate) fn open(&self) -> bool {
        self.sent && self.received
    }

    /// The room an encode may use in `out`, after a stream's prefix: never
    /// more than `max_frame`, so a frame over the limit overflows instead of
    /// being committed.
    pub(crate) fn split_out<'o>(&self, out: &'o mut [u8]) -> (&'o mut [u8], &'o mut [u8]) {
        let prefix = if self.stream { PREFIX_LEN } else { 0 };
        let (head, rest) = out.split_at_mut(prefix.min(out.len()));
        let room = rest.len().min(self.max_frame);
        (head, &mut rest[..room])
    }

    /// Finishes a frame of `len` bytes written after `head`: its prefix, in
    /// stream mode, and the total written.
    pub(crate) fn finish(&self, head: &mut [u8], len: usize) -> usize {
        if !self.stream {
            return len;
        }
        let mut prefix = SliceBuf::new(head);
        stream::put_prefix(len, self.max_frame, &mut prefix)
            .expect("the frame was written within max_frame");
        assert!(
            !prefix.overflowed(),
            "a frame was written without room for its prefix"
        );
        PREFIX_LEN + len
    }

    /// The status for a frame that needs `len` bytes and didn't fit: too
    /// large for the connection, or too large for the caller's buffer.
    pub(crate) fn too_small(&self, len: usize, written: &mut usize) -> comline_status {
        if len > self.max_frame {
            return COMLINE_ERR_FRAME_TOO_LARGE;
        }
        *written = if self.stream { PREFIX_LEN + len } else { len };
        COMLINE_BUFFER_TOO_SMALL
    }
}

/// A [`CountBuf`] that also says it overflowed, so
/// [`State::encode_request_body`] counts the whole frame and commits nothing.
pub(crate) struct Measure(pub(crate) CountBuf);

impl BufMut for Measure {
    fn put_slice(&mut self, bytes: &[u8]) {
        self.0.put_slice(bytes);
    }

    fn overflowed(&self) -> bool {
        true
    }
}
