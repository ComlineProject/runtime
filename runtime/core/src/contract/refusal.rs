use core::fmt;

/// Why a `comline@2` handshake refused a connection, or one protocol on it
/// (runtime#15). The codes are stable: a code is never renumbered or
/// reused, and a new reason takes the next free one.
///
/// | code | name | scope |
/// |---|---|---|
/// | 1 | `malformed` | connection |
/// | 2 | `handshake_version` | connection |
/// | 3 | `exchange_mismatch` | connection |
/// | 4 | `package_mismatch` | connection |
/// | 5 | `encoding_mismatch` | connection |
/// | 6 | `line_mismatch` | protocol |
/// | 7 | `same_version_differs` | protocol |
/// | 8 | `newer_consumer` | protocol |
/// | 9 | `unknown_protocol` | protocol |
///
/// A protocol-scope reason refuses that protocol alone under
/// `scope = protocol`, and the whole connection under `scope = package`
/// (`unknown_protocol` excepted: it never refuses more than its protocol).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "alloc", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "alloc", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum Reason {
    /// The peer's handshake message didn't parse.
    Malformed = 1,
    /// The peer speaks another version of the handshake (a `comline@1`
    /// frame, say).
    HandshakeVersion = 2,
    /// The two ends declare different `exchange` parameters.
    ExchangeMismatch = 3,
    /// The two ends were built from different packages (their package ids
    /// differ).
    PackageMismatch = 4,
    /// The two ends' encodings (format, framing, handshake, each by its name
    /// and parameter hash) differ.
    EncodingMismatch = 5,
    /// The protocol broke between the two ends' versions: its lines differ.
    LineMismatch = 6,
    /// Both ends claim the same package version, but the protocol's wire
    /// shape differs, under `same_version_differs = refuse`.
    SameVersionDiffers = 7,
    /// The consumer calls functions the provider lacks, under
    /// `newer_consumer = refuse`.
    NewerConsumer = 8,
    /// One end has the protocol and the other doesn't.
    UnknownProtocol = 9,
}

impl Reason {
    /// Every reason, in code order.
    pub const ALL: [Reason; 9] = [
        Reason::Malformed,
        Reason::HandshakeVersion,
        Reason::ExchangeMismatch,
        Reason::PackageMismatch,
        Reason::EncodingMismatch,
        Reason::LineMismatch,
        Reason::SameVersionDiffers,
        Reason::NewerConsumer,
        Reason::UnknownProtocol,
    ];

    /// Its stable code, as the wire carries it.
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Reason> {
        Reason::ALL.into_iter().find(|r| r.code() == code)
    }

    /// Whether it's about one protocol, whose number a refusal then names.
    pub fn per_protocol(self) -> bool {
        matches!(
            self,
            Reason::LineMismatch
                | Reason::SameVersionDiffers
                | Reason::NewerConsumer
                | Reason::UnknownProtocol
        )
    }

    /// Its stable name, as the docs and the conformance vectors spell it.
    pub fn name(self) -> &'static str {
        match self {
            Reason::Malformed => "malformed",
            Reason::HandshakeVersion => "handshake_version",
            Reason::ExchangeMismatch => "exchange_mismatch",
            Reason::PackageMismatch => "package_mismatch",
            Reason::EncodingMismatch => "encoding_mismatch",
            Reason::LineMismatch => "line_mismatch",
            Reason::SameVersionDiffers => "same_version_differs",
            Reason::NewerConsumer => "newer_consumer",
            Reason::UnknownProtocol => "unknown_protocol",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Which end of a connection: the one that opened it (the consumer) or the
/// one that accepted it (the provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "alloc", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "alloc", serde(rename_all = "snake_case"))]
pub enum Side {
    Initiator = 0,
    Responder = 1,
}

impl Side {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Side> {
        match code {
            0 => Some(Side::Initiator),
            1 => Some(Side::Responder),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Initiator => "initiator",
            Side::Responder => "responder",
        }
    }
}
