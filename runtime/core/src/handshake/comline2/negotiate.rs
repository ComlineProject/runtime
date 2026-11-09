use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::contract::{Reason, RuntimeError, Side};

use super::facts::{Hello, PackageVersion, Protocol};
use super::message::{Answer, Answered, Message, Refusal, RefusedProtocol, Status, Verdict};
use super::{Exchange, NewerConsumer, Params, SameVersionDiffers, Scope};

/// What one of this end's protocols came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolState {
    /// Usable. `peer_lacks`: this end's functions the peer doesn't have;
    /// calls to them fail locally ([`Negotiated::check_call`]).
    Accepted { peer_lacks: Vec<u16> },
    /// Refused on its own (`scope = protocol`): every call to it fails
    /// locally.
    Refused { reason: Reason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolOutcome {
    pub number: u16,
    pub state: ProtocolState,
}

/// What a connection agreed: per protocol of this end's, whether it's
/// usable and which of its functions the peer lacks; and the protocols that
/// connected despite a shape differing at the same version
/// (`same_version_differs = warn`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiated {
    pub peer_version: PackageVersion,
    /// Ascending by number.
    pub protocols: Vec<ProtocolOutcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<u16>,
}

impl Negotiated {
    /// Whether this end may call `function` of `protocol`: `Ok` to send it,
    /// or [`RuntimeError::Unavailable`] when the peer lacks the function or
    /// the protocol was refused. Generated code calls this before it writes
    /// a request, so a call the peer can't take never reaches the wire.
    pub fn check_call(&self, protocol: u16, function: u16) -> Result<(), RuntimeError> {
        let unavailable = RuntimeError::Unavailable;
        let p = self
            .protocols
            .binary_search_by_key(&protocol, |p| p.number)
            .map_err(|_| unavailable)?;
        match &self.protocols[p].state {
            ProtocolState::Accepted { peer_lacks }
                if peer_lacks.binary_search(&function).is_err() =>
            {
                Ok(())
            }
            _ => Err(unavailable),
        }
    }

    /// Takes in the protocols the peer's verdict refused.
    fn apply(&mut self, refused: &[RefusedProtocol]) {
        for r in refused {
            if let Ok(i) = self.protocols.binary_search_by_key(&r.number, |p| p.number) {
                if matches!(self.protocols[i].state, ProtocolState::Accepted { .. }) {
                    self.protocols[i].state = ProtocolState::Refused { reason: r.reason };
                }
            }
        }
    }
}

/// One step's result: the message to send (if any), and what this end
/// concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub reply: Option<Message>,
    pub result: Result<Negotiated, Refusal>,
}

fn difference(a: &[u16], b: &[u16]) -> Vec<u16> {
    a.iter()
        .copied()
        .filter(|n| b.binary_search(n).is_err())
        .collect()
}

/// `comline@2` for one end, sans-IO: its parameters and its [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comline2 {
    pub params: Params,
    pub hello: Hello,
}

impl Comline2 {
    /// `hello` is put in canonical order, and its `exchange` set from
    /// `params`.
    pub fn new(params: Params, hello: Hello) -> Self {
        let mut hello = hello.canonical();
        hello.exchange = params.exchange;
        Self { params, hello }
    }

    /// The [`Hello`] this end sends.
    pub fn hello_message(&self) -> Message {
        Message::Hello(self.hello.clone())
    }

    fn refusal(&self, reason: Reason, side: Side, protocol: Option<u16>) -> Refusal {
        Refusal {
            reason,
            side,
            protocol,
        }
    }

    /// The checks before any protocol: the exchange, the package, the
    /// encodings.
    fn check_connection(&self, peer: &Hello, me: Side) -> Result<(), Refusal> {
        let refuse = |reason| Err(self.refusal(reason, me, None));
        if peer.exchange != self.params.exchange {
            return refuse(Reason::ExchangeMismatch);
        }
        if peer.package_id != self.hello.package_id {
            return refuse(Reason::PackageMismatch);
        }
        if peer.encodings != self.hello.encodings {
            return refuse(Reason::EncodingMismatch);
        }
        Ok(())
    }

    /// Judge one protocol both ends have, knowing both hellos. `Ok(warn)`.
    fn judge_protocol(
        &self,
        me: Side,
        peer_version: &PackageVersion,
        mine: &Protocol,
        theirs: &Protocol,
    ) -> Result<bool, Reason> {
        if mine.line != theirs.line {
            return Err(Reason::LineMismatch);
        }
        let mut warn = false;
        if mine.shape_hash != theirs.shape_hash && *peer_version == self.hello.version {
            match self.params.same_version_differs {
                SameVersionDiffers::Refuse => return Err(Reason::SameVersionDiffers),
                SameVersionDiffers::Warn => warn = true,
            }
        }
        let (consumer, provider) = match me {
            Side::Initiator => (mine, theirs),
            Side::Responder => (theirs, mine),
        };
        if self.params.newer_consumer == NewerConsumer::Refuse
            && !difference(&consumer.functions, &provider.functions).is_empty()
        {
            return Err(Reason::NewerConsumer);
        }
        Ok(warn)
    }

    /// Apply `scope` to the per-protocol outcomes: a refusal of the whole
    /// connection, if any.
    fn scoped(&self, me: Side, protocols: &[(u16, Option<Reason>)]) -> Result<(), Refusal> {
        let refused = || protocols.iter().filter_map(|(n, r)| r.map(|r| (*n, r)));
        if self.params.scope == Scope::Package {
            if let Some((n, r)) = refused().find(|(_, r)| *r != Reason::UnknownProtocol) {
                return Err(self.refusal(r, me, Some(n)));
            }
        }
        if !protocols.is_empty() && protocols.iter().all(|(_, r)| r.is_some()) {
            let (n, r) = refused().next().unwrap();
            return Err(self.refusal(r, me, Some(n)));
        }
        Ok(())
    }

    /// Judge the peer's [`Hello`] knowing both: the responder under
    /// `offer`, and both ends under `symmetric`. Every protocol either end
    /// has is judged; one only one end has is `unknown_protocol`.
    fn judge(
        &self,
        me: Side,
        peer: &Hello,
    ) -> Result<(Negotiated, Vec<(u16, Option<Reason>, bool, Vec<u16>)>), Refusal> {
        self.check_connection(peer, me)?;
        let mut numbers: Vec<u16> = self
            .hello
            .protocols
            .iter()
            .chain(&peer.protocols)
            .map(|p| p.number)
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        // (number, refusal, shape differs, the peer's functions this end lacks)
        let mut judged = Vec::new();
        let mut negotiated = Negotiated {
            peer_version: peer.version.clone(),
            protocols: Vec::new(),
            warnings: Vec::new(),
        };
        for n in numbers {
            let (mine, theirs) = (self.hello.protocol(n), peer.protocol(n));
            let (Some(mine), Some(theirs)) = (mine, theirs) else {
                judged.push((n, Some(Reason::UnknownProtocol), false, Vec::new()));
                if mine.is_some() {
                    negotiated.protocols.push(ProtocolOutcome {
                        number: n,
                        state: ProtocolState::Refused {
                            reason: Reason::UnknownProtocol,
                        },
                    });
                }
                continue;
            };
            let missing = difference(&theirs.functions, &mine.functions);
            let differs = mine.shape_hash != theirs.shape_hash;
            match self.judge_protocol(me, &peer.version, mine, theirs) {
                Ok(warn) => {
                    if warn {
                        negotiated.warnings.push(n);
                    }
                    judged.push((n, None, differs, missing));
                    negotiated.protocols.push(ProtocolOutcome {
                        number: n,
                        state: ProtocolState::Accepted {
                            peer_lacks: difference(&mine.functions, &theirs.functions),
                        },
                    });
                }
                Err(reason) => {
                    judged.push((n, Some(reason), differs, missing));
                    negotiated.protocols.push(ProtocolOutcome {
                        number: n,
                        state: ProtocolState::Refused { reason },
                    });
                }
            }
        }
        let outcomes: Vec<(u16, Option<Reason>)> = judged.iter().map(|j| (j.0, j.1)).collect();
        self.scoped(me, &outcomes)?;
        Ok((negotiated, judged))
    }

    fn peer_hello(bytes: &[u8]) -> Result<Hello, Reason> {
        match Message::decode(bytes)? {
            Message::Hello(h) => Ok(h),
            _ => Err(Reason::Malformed),
        }
    }

    /// `offer`, the responder: judge the initiator's [`Hello`] and answer.
    /// An accepted result still waits on the initiator's [`Verdict`]
    /// ([`finish`](Self::finish)).
    pub fn answer_offer(&self, offer: &[u8]) -> Step {
        let me = Side::Responder;
        let refuse = |r: Refusal| Step {
            reply: Some(Message::Answer(Answer::Refuse(r))),
            result: Err(r),
        };
        let peer = match Self::peer_hello(offer) {
            Ok(peer) => peer,
            Err(reason) => return refuse(self.refusal(reason, me, None)),
        };
        if self.params.exchange != Exchange::Offer || peer.exchange != Exchange::Offer {
            return refuse(self.refusal(Reason::ExchangeMismatch, me, None));
        }
        match self.judge(me, &peer) {
            Err(r) => refuse(r),
            Ok((negotiated, judged)) => {
                // Answer every offered protocol, in the offer's order.
                let protocols = peer
                    .protocols
                    .iter()
                    .map(|p| {
                        let j = judged.iter().find(|j| j.0 == p.number).unwrap();
                        Answered {
                            number: p.number,
                            status: match j.1 {
                                Some(reason) => Status::Refused { reason },
                                None => Status::Accepted {
                                    shape_differs: j.2,
                                    missing: j.3.clone(),
                                },
                            },
                        }
                    })
                    .collect();
                Step {
                    reply: Some(Message::Answer(Answer::Accept {
                        version: self.hello.version.clone(),
                        protocols,
                    })),
                    result: Ok(negotiated),
                }
            }
        }
    }

    /// `offer`, the initiator: judge the responder's [`Answer`] by this
    /// end's own policy, and give the [`Verdict`] to send (none when the
    /// responder refused).
    pub fn conclude_answer(&self, answer: &[u8]) -> Step {
        let me = Side::Initiator;
        let refuse = |r: Refusal| Step {
            reply: Some(Message::Verdict(Verdict::Refuse(r))),
            result: Err(r),
        };
        let (version, answered) = match Message::decode(answer) {
            Ok(Message::Answer(Answer::Refuse(r))) => {
                return Step {
                    reply: None,
                    result: Err(r),
                }
            }
            Ok(Message::Answer(Answer::Accept { version, protocols })) => (version, protocols),
            // A symmetric responder sends a hello where an answer belongs.
            Ok(Message::Hello(_)) => {
                return refuse(self.refusal(Reason::ExchangeMismatch, me, None))
            }
            Ok(Message::Verdict(_)) => return refuse(self.refusal(Reason::Malformed, me, None)),
            Err(reason) => return refuse(self.refusal(reason, me, None)),
        };
        let offered: Vec<u16> = self.hello.protocols.iter().map(|p| p.number).collect();
        if answered.iter().map(|a| a.number).collect::<Vec<_>>() != offered {
            return refuse(self.refusal(Reason::Malformed, me, None));
        }
        let mut negotiated = Negotiated {
            peer_version: version.clone(),
            protocols: Vec::new(),
            warnings: Vec::new(),
        };
        let mut outcomes = Vec::new();
        let mut own = Vec::new();
        for (mine, a) in self.hello.protocols.iter().zip(&answered) {
            let state = match &a.status {
                Status::Refused { reason } => Err(*reason),
                Status::Accepted {
                    shape_differs,
                    missing,
                } => {
                    if missing
                        .iter()
                        .any(|n| mine.functions.binary_search(n).is_err())
                    {
                        return refuse(self.refusal(Reason::Malformed, me, None));
                    }
                    let same = *shape_differs && version == self.hello.version;
                    if same && self.params.same_version_differs == SameVersionDiffers::Refuse {
                        own.push(RefusedProtocol {
                            number: a.number,
                            reason: Reason::SameVersionDiffers,
                        });
                        Err(Reason::SameVersionDiffers)
                    } else if !missing.is_empty()
                        && self.params.newer_consumer == NewerConsumer::Refuse
                    {
                        own.push(RefusedProtocol {
                            number: a.number,
                            reason: Reason::NewerConsumer,
                        });
                        Err(Reason::NewerConsumer)
                    } else {
                        if same {
                            negotiated.warnings.push(a.number);
                        }
                        Ok(missing.clone())
                    }
                }
            };
            outcomes.push((a.number, state.as_ref().err().copied()));
            negotiated.protocols.push(ProtocolOutcome {
                number: a.number,
                state: match state {
                    Ok(peer_lacks) => ProtocolState::Accepted { peer_lacks },
                    Err(reason) => ProtocolState::Refused { reason },
                },
            });
        }
        if let Err(r) = self.scoped(me, &outcomes) {
            return refuse(r);
        }
        Step {
            reply: Some(Message::Verdict(Verdict::Accept { refused: own })),
            result: Ok(negotiated),
        }
    }

    /// `symmetric`, either end: judge the peer's [`Hello`] and give this
    /// end's [`Verdict`]. An accepted result still waits on the peer's
    /// ([`finish`](Self::finish)).
    pub fn judge_hello(&self, me: Side, peer: &[u8]) -> Step {
        let refuse = |r: Refusal| Step {
            reply: Some(Message::Verdict(Verdict::Refuse(r))),
            result: Err(r),
        };
        let peer = match Message::decode(peer) {
            Ok(Message::Hello(peer)) => peer,
            // An offer responder refusing (our hello isn't an offer), or a
            // peer that refused before judging: take its refusal.
            Ok(Message::Answer(Answer::Refuse(r)) | Message::Verdict(Verdict::Refuse(r))) => {
                return Step {
                    reply: None,
                    result: Err(r),
                }
            }
            Ok(_) => return refuse(self.refusal(Reason::Malformed, me, None)),
            Err(reason) => return refuse(self.refusal(reason, me, None)),
        };
        if self.params.exchange != Exchange::Symmetric {
            return refuse(self.refusal(Reason::ExchangeMismatch, me, None));
        }
        match self.judge(me, &peer) {
            Err(r) => refuse(r),
            Ok((negotiated, judged)) => {
                let refused = judged
                    .iter()
                    .filter_map(|j| {
                        j.1.map(|reason| RefusedProtocol {
                            number: j.0,
                            reason,
                        })
                    })
                    .collect();
                Step {
                    reply: Some(Message::Verdict(Verdict::Accept { refused })),
                    result: Ok(negotiated),
                }
            }
        }
    }

    /// Take in the peer's [`Verdict`]: its refusal, or the protocols it
    /// refused. A message that isn't a verdict is `malformed`, refused by
    /// `me`.
    pub fn finish(
        me: Side,
        mut negotiated: Negotiated,
        verdict: &[u8],
    ) -> Result<Negotiated, Refusal> {
        match Message::decode(verdict) {
            Ok(Message::Verdict(Verdict::Accept { refused })) => {
                negotiated.apply(&refused);
                Ok(negotiated)
            }
            Ok(Message::Verdict(Verdict::Refuse(r))) => Err(r),
            Ok(_) => Err(Refusal {
                reason: Reason::Malformed,
                side: me,
                protocol: None,
            }),
            Err(reason) => Err(Refusal {
                reason,
                side: me,
                protocol: None,
            }),
        }
    }
}
