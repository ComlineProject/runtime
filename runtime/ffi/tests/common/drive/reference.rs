use super::abi::{Encoded, Fed};
use super::status::bit;
use super::Config;
use comline_runtime::connection::{Event, State};
use comline_runtime::contract::{
    ConnectionFraming, Datagram2Framing, Datagram3Framing, Envelope, HandshakeFrame, RequestCall,
    RuntimeError, SliceBuf, FRAMING_DATAGRAM2, FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime_ffi::*;

/// The connection framings, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fr {
    Datagram2,
    Datagram3,
    JsonRpc,
}

/// Runs `$body` with `$f` bound to the framing as its concrete type.
macro_rules! framed {
    ($fr:expr, |$f:ident| $body:expr) => {
        match $fr {
            Fr::Datagram2 => {
                let $f = &Datagram2Framing;
                $body
            }
            Fr::Datagram3 => {
                let $f = &Datagram3Framing;
                $body
            }
            Fr::JsonRpc => {
                let $f = &JsonRpcFraming;
                $body
            }
        }
    };
}

/// What the ABI promises, from the core's public API: `State` and the
/// framings for the frames, `HandshakeFrame` for the handshake, sizes from
/// encoding into a `Vec`, and the stream prefix read by hand.
pub(super) struct Reference {
    state: State,
    fr: Fr,
    ours: Option<HandshakeFrame>,
    sent: bool,
    received: bool,
    closed: bool,
    stream: bool,
    max_frame: usize,
    /// The id `State` hands out next: 0, then one past the last it gave.
    next_id: u64,
}

/// Where `part` lies in `input`, when it borrows from it.
fn span_of(input: &[u8], part: &[u8]) -> Option<comline_span> {
    let base = input.as_ptr() as usize;
    let at = part.as_ptr() as usize;
    if at < base || at + part.len() > base + input.len() {
        return None;
    }
    Some(comline_span {
        offset: u32::try_from(at - base).ok()?,
        len: u32::try_from(part.len()).ok()?,
    })
}

impl Reference {
    /// The connection, or every refusal that applies to `config`.
    pub(super) fn new(config: &Config) -> Result<Self, u32> {
        let mut either = 0;
        let framing = std::str::from_utf8(&config.framing).ok();
        let wire_format = std::str::from_utf8(&config.wire_format).ok();
        if framing.is_none() || wire_format.is_none() {
            either |= bit(COMLINE_ERR_INVALID_ARGUMENT);
        }
        if config.stream > 1
            || config.unordered > 1
            || config.reserved != 0
            || config.max_frame > COMLINE_MAX_FRAME_CEILING
            || !matches!(
                config.handshake,
                COMLINE_HANDSHAKE_COMLINE | COMLINE_HANDSHAKE_NONE
            )
        {
            either |= bit(COMLINE_ERR_INVALID_ARGUMENT);
        }
        let fr = match framing {
            Some(FRAMING_DATAGRAM2) => Some(Fr::Datagram2),
            Some(FRAMING_DATAGRAM3) => Some(Fr::Datagram3),
            Some(FRAMING_JSONRPC) => Some(Fr::JsonRpc),
            Some(_) => {
                either |= bit(COMLINE_ERR_UNKNOWN_FRAMING);
                None
            }
            None => None,
        };
        let (Some(fr), Some(framing), Some(wire_format), 0) = (fr, framing, wire_format, either)
        else {
            return Err(either);
        };
        let ours = (config.handshake == COMLINE_HANDSHAKE_COMLINE).then(|| {
            HandshakeFrame::new(config.ir_hash, wire_format, framing, config.capabilities)
        });
        let capacity = config.max_waiting as usize;
        Ok(Self {
            state: if config.unordered == 1 {
                State::unordered_with_capacity(capacity)
            } else {
                State::with_capacity(capacity)
            },
            fr,
            ours,
            sent: ours.is_none(),
            received: ours.is_none(),
            closed: false,
            stream: config.stream == 1,
            max_frame: match config.max_frame {
                0 => COMLINE_DEFAULT_MAX_FRAME as usize,
                n => n as usize,
            },
            next_id: 0,
        })
    }

    fn refused(either: u32) -> Encoded {
        Encoded {
            status: COMLINE_ERR_STATE,
            written: 0,
            request_id: 0,
            bytes: Vec::new(),
            either,
        }
    }

    fn status(status: comline_status, written: usize) -> Encoded {
        Encoded {
            status,
            written,
            request_id: 0,
            bytes: Vec::new(),
            either: 0,
        }
    }

    /// The outcome of sending `frame` with `cap` bytes of room: too large
    /// for the connection, too large for `cap`, or written behind its
    /// prefix.
    fn sized(&self, frame: &[u8], cap: usize) -> Encoded {
        if frame.len() > self.max_frame {
            return Self::status(COMLINE_ERR_FRAME_TOO_LARGE, 0);
        }
        let mut bytes = Vec::new();
        if self.stream {
            bytes.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        }
        bytes.extend_from_slice(frame);
        if bytes.len() > cap {
            return Self::status(COMLINE_BUFFER_TOO_SMALL, bytes.len());
        }
        Encoded {
            status: COMLINE_OK,
            written: bytes.len(),
            request_id: 0,
            bytes,
            either: 0,
        }
    }

    pub(super) fn start(&mut self, cap: usize) -> Encoded {
        if self.closed {
            return Self::refused(bit(COMLINE_ERR_STATE));
        }
        let Some(ours) = self.ours else {
            return Self::status(COMLINE_OK, 0);
        };
        if self.sent {
            return Self::refused(bit(COMLINE_ERR_STATE));
        }
        let mut frame = Vec::new();
        ours.encode(&mut frame);
        let got = self.sized(&frame, cap);
        self.sent |= got.status == COMLINE_OK;
        got
    }

    /// The refusals that apply before anything is framed.
    fn before(&self, bad_argument: bool) -> u32 {
        let mut either = 0;
        if self.closed || !(self.sent && self.received) {
            either |= bit(COMLINE_ERR_STATE);
        }
        if bad_argument && !self.closed {
            either |= bit(COMLINE_ERR_INVALID_ARGUMENT);
        }
        either
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn request(
        &mut self,
        protocol: u16,
        call_id: u16,
        name: &[u8],
        expects_reply: u8,
        body: &[u8],
        cap: usize,
    ) -> Result<Encoded, String> {
        let name = std::str::from_utf8(name).ok();
        let either = self.before(name.is_none() || expects_reply > 1);
        let (Some(name), 0) = (name, either) else {
            return Ok(Self::refused(either));
        };
        let expects = expects_reply == 1;
        // No room at all: `Busy` if the call can't wait, and otherwise a
        // frame that doesn't fit, which commits nothing.
        let probe = framed!(self.fr, |f| self.state.encode_request_body(
            f,
            protocol,
            call_id,
            name,
            body,
            expects,
            &mut SliceBuf::new(&mut [])
        ));
        match probe {
            Err(RuntimeError::Busy) => return Ok(Self::status(COMLINE_ERR_BUSY, 0)),
            Err(RuntimeError::BufferTooSmall) => {}
            other => {
                return Err(format!(
                    "reference: an empty buffer took a frame: {other:?}"
                ))
            }
        }
        let mut frame = Vec::new();
        framed!(self.fr, |f| f.encode_request_body(
            protocol,
            call_id,
            name,
            self.next_id,
            !expects,
            body,
            &mut frame
        ));
        let mut got = self.sized(&frame, cap);
        if got.status != COMLINE_OK {
            return Ok(got);
        }
        let mut committed = Vec::new();
        let id = framed!(self.fr, |f| self.state.encode_request_body(
            f,
            protocol,
            call_id,
            name,
            body,
            expects,
            &mut committed
        ))
        .map_err(|e| format!("reference: {e:?}"))?;
        ensure!(
            id == self.next_id && committed == frame,
            "reference: State gave id {id} and {committed:02x?}, after {frame:02x?} for {}",
            self.next_id
        );
        self.next_id = id.wrapping_add(1);
        got.request_id = id;
        Ok(got)
    }

    pub(super) fn reply(
        &mut self,
        protocol: u16,
        request_id: u64,
        envelope: u8,
        error_id: u16,
        body: &[u8],
        cap: usize,
    ) -> Encoded {
        let envelope = match envelope {
            COMLINE_ENVELOPE_OK => Some(Envelope::Ok(body)),
            COMLINE_ENVELOPE_ERR => Some(Envelope::Err { id: error_id, body }),
            COMLINE_ENVELOPE_FAULT if body.is_empty() => Some(Envelope::Fault),
            _ => None,
        };
        let either = self.before(envelope.is_none());
        let (Some(envelope), 0) = (envelope, either) else {
            return Self::refused(either);
        };
        let mut frame = Vec::new();
        framed!(self.fr, |f| self
            .state
            .encode_reply(f, protocol, request_id, &envelope, &mut frame));
        self.sized(&frame, cap)
    }

    pub(super) fn abandon(&mut self, request_id: u64) -> comline_status {
        if self.closed {
            return COMLINE_ERR_STATE;
        }
        self.state.abandon(request_id);
        COMLINE_OK
    }

    pub(super) fn is_waiting(&self) -> (comline_status, u8) {
        if self.closed {
            return (COMLINE_ERR_STATE, 0);
        }
        (COMLINE_OK, u8::from(self.state.is_waiting()))
    }

    pub(super) fn feed(&mut self, input: &[u8]) -> Result<Fed, String> {
        let mut fed = Fed {
            status: COMLINE_OK,
            consumed: 0,
            ev: comline_event::default(),
        };
        if self.closed {
            fed.status = COMLINE_ERR_STATE;
            return Ok(fed);
        }
        let (start, len) = if self.stream {
            let Some(prefix) = input.first_chunk::<4>() else {
                fed.status = COMLINE_NEED_MORE;
                return Ok(fed);
            };
            let len = u32::from_le_bytes(*prefix) as usize;
            if len > self.max_frame {
                self.closed = true;
                fed.status = COMLINE_ERR_FRAME_TOO_LARGE;
                return Ok(fed);
            }
            if 4 + len > input.len() {
                fed.status = COMLINE_NEED_MORE;
                fed.ev.frame.len = (4 + len) as u32;
                return Ok(fed);
            }
            (4, len)
        } else {
            if input.len() > self.max_frame {
                fed.status = COMLINE_ERR_FRAME_TOO_LARGE;
                return Ok(fed);
            }
            (0, input.len())
        };
        let frame = &input[start..start + len];
        fed.consumed = start + len;
        fed.ev.frame = span_of(input, frame).expect("the frame is in the input");

        if !self.received {
            let ours = self.ours.expect("only a handshake leaves `received` unset");
            match ours.check_bytes(frame) {
                Ok(peer) => {
                    self.received = true;
                    fed.ev.kind = COMLINE_EVENT_HANDSHAKE_OK;
                    fed.ev.capabilities = peer.capabilities;
                }
                Err(RuntimeError::Handshake) => {
                    self.closed = true;
                    fed.status = COMLINE_ERR_HANDSHAKE;
                }
                Err(other) => return Err(format!("reference: the handshake raised {other:?}")),
            }
            return Ok(fed);
        }

        let event = match framed!(self.fr, |f| self.state.receive(f, frame)) {
            Ok(event) => event,
            Err(RuntimeError::Framing) => {
                fed.status = COMLINE_ERR_FRAMING;
                return Ok(fed);
            }
            Err(RuntimeError::PeerRefused) => {
                fed.status = COMLINE_ERR_PEER_REFUSED;
                return Ok(fed);
            }
            Err(other) => return Err(format!("reference: receive raised {other:?}")),
        };
        let ev = &mut fed.ev;
        // A body that doesn't borrow the frame is JSON-RPC's absent one.
        let body = |ev: &mut comline_event, part: &[u8]| match span_of(input, part) {
            Some(span) => ev.body = span,
            None => ev.flags |= COMLINE_EVENT_BODY_ABSENT,
        };
        match event {
            Event::Request {
                protocol,
                request,
                one_way,
            } => {
                ev.kind = COMLINE_EVENT_REQUEST;
                if let Some(protocol) = protocol {
                    ev.flags |= COMLINE_EVENT_HAS_PROTOCOL;
                    ev.protocol = protocol;
                }
                if one_way {
                    ev.flags |= COMLINE_EVENT_ONE_WAY;
                }
                match request.call {
                    RequestCall::Id(id) => ev.call_id = id,
                    RequestCall::Name(name) => {
                        let (written, escaped) = name.as_written();
                        ev.flags |= COMLINE_EVENT_CALL_BY_NAME;
                        if escaped {
                            ev.flags |= COMLINE_EVENT_NAME_ESCAPED;
                        }
                        ev.call_name = span_of(input, written.as_bytes())
                            .ok_or("reference: a method name not in the frame")?;
                    }
                }
                ev.request_id = request.request_id;
                body(ev, request.params);
            }
            Event::Reply {
                request_id,
                envelope,
            } => {
                ev.kind = COMLINE_EVENT_REPLY;
                ev.request_id = request_id;
                match envelope {
                    Envelope::Ok(payload) => {
                        ev.envelope = COMLINE_ENVELOPE_OK;
                        body(ev, payload);
                    }
                    Envelope::Err { id, body: fields } => {
                        ev.envelope = COMLINE_ENVELOPE_ERR;
                        ev.error_id = id;
                        body(ev, fields);
                    }
                    Envelope::Fault => ev.envelope = COMLINE_ENVELOPE_FAULT,
                }
            }
            Event::Stale => ev.kind = COMLINE_EVENT_STALE,
        }
        Ok(fed)
    }
}
