//! `comline@2`: the compatibility-checking handshake (runtime#15).
//!
//! Where `comline@1` refuses any peer whose IR differs, `comline@2` compares
//! what core's history records about each version (`comline_core::compat`):
//! the package id, each protocol's wire number, **line** (the release it
//! last broke at) and wire-shape hash, each function's wire number, and each
//! encoding's parameter hash. Lines decide compatibility; the shape hash
//! catches two ends that claim the same version but differ. The namespace
//! is never compared.
//!
//! Four parameters, defaults first. `exchange` is a wire parameter, which
//! both ends must share (it is part of the handshake's parameter hash and
//! of every message); the others are each end's own policy.
//!
//! - `scope = protocol | package`: a protocol-scope refusal refuses that
//!   protocol alone, or the whole connection.
//! - `newer_consumer = degrade | refuse`: when the consumer has functions
//!   the provider lacks, connect anyway (calls to them fail locally with
//!   [`RuntimeError::Unavailable`](crate::contract::RuntimeError::Unavailable), never reaching the wire), or refuse.
//! - `same_version_differs = refuse | warn`: when both claim the same
//!   package version but a protocol's shape hash differs.
//! - `exchange = offer | symmetric`: the initiator offers and the responder
//!   answers, or both declare and both judge.
//!
//! The messages and their layout are in [`Message`]; the sans-IO steps are
//! on [`Comline2`], and [`Comline2Handshake`] runs them over a
//! [`Transport`](crate::transport::Transport) (and, with `async`, an `AsyncTransport`).

use serde::{Deserialize, Serialize};

mod driver;
mod facts;
mod message;
mod negotiate;

pub use driver::Comline2Handshake;
pub use facts::{Encoding, EncodingKind, Hash128, Hello, PackageVersion, Protocol, Release};
pub use message::{Answer, Answered, Message, Refusal, RefusedProtocol, Status, Verdict};
pub use negotiate::{Comline2, Negotiated, ProtocolOutcome, ProtocolState, Step};

/// `scope`: what a protocol-scope refusal refuses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// That protocol alone; the connection carries the others.
    #[default]
    Protocol,
    /// The whole connection.
    Package,
}

/// `newer_consumer`: what to do when the consumer has functions the
/// provider lacks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewerConsumer {
    /// Connect; calls to the missing functions fail locally.
    #[default]
    Degrade,
    /// Refuse the protocol ([`Reason::NewerConsumer`](crate::contract::Reason::NewerConsumer)).
    Refuse,
}

/// `same_version_differs`: what to do when both ends claim the same package
/// version but a protocol's wire shape differs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameVersionDiffers {
    /// Refuse the protocol ([`Reason::SameVersionDiffers`](crate::contract::Reason::SameVersionDiffers)).
    #[default]
    Refuse,
    /// Connect, and report it in [`Negotiated::warnings`].
    Warn,
}

/// `exchange`: who sends what. A wire parameter: both ends must share it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exchange {
    /// The initiator sends a [`Hello`]; the responder judges it and sends an
    /// [`Answer`]; the initiator judges that and sends a [`Verdict`].
    #[default]
    Offer,
    /// Both send a [`Hello`], both judge the other's, both send a
    /// [`Verdict`].
    Symmetric,
}

impl Exchange {
    fn code(self) -> u8 {
        match self {
            Exchange::Offer => 0,
            Exchange::Symmetric => 1,
        }
    }

    pub(super) fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Exchange::Offer),
            1 => Some(Exchange::Symmetric),
            _ => None,
        }
    }
}

/// The four parameters, as `config.idp` declares them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub scope: Scope,
    pub newer_consumer: NewerConsumer,
    pub same_version_differs: SameVersionDiffers,
    pub exchange: Exchange,
}
