use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use super::Exchange;

/// A 128-bit hash: the first 16 bytes of core's blake3 shape and parameter
/// hashes. Hex in JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash128(pub [u8; 16]);

impl Hash128 {
    /// From hex, as core's facts write a hash: the first 32 digits, so a
    /// full 64-digit blake3 hash is cut to 128 bits.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.as_bytes().get(..32)?;
        let mut out = [0u8; 16];
        for (i, pair) in hex.chunks(2).enumerate() {
            let digit = |b: u8| (b as char).to_digit(16);
            out[i] = (digit(pair[0])? * 16 + digit(pair[1])?) as u8;
        }
        Some(Hash128(out))
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(32);
        for b in self.0 {
            s.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
            s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap());
        }
        s
    }
}

impl Serialize for Hash128 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hash128 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        if hex.len() != 32 {
            return Err(serde::de::Error::custom("a 128-bit hash is 32 hex digits"));
        }
        Hash128::from_hex(&hex).ok_or_else(|| serde::de::Error::custom("not hex"))
    }
}

/// A release, `major.minor.patch`: a protocol's line.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Release {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Release {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

/// A package version: a release, and a pre-release (`dev.2`), empty for a
/// release.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVersion {
    #[serde(flatten)]
    pub release: Release,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pre: String,
}

impl PackageVersion {
    pub fn release(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            release: Release::new(major, minor, patch),
            pre: String::new(),
        }
    }
}

/// What an encoding is: a format, a framing, or the handshake itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodingKind {
    Format = 0,
    Framing = 1,
    Handshake = 2,
}

impl EncodingKind {
    pub(super) fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(EncodingKind::Format),
            1 => Some(EncodingKind::Framing),
            2 => Some(EncodingKind::Handshake),
            _ => None,
        }
    }
}

/// An encoding in play: its kind and core's parameter hash, which covers
/// its name and parameters (`EncodingFacts.param_hash`). The handshake's
/// own entry covers `exchange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Encoding {
    pub kind: EncodingKind,
    pub param_hash: Hash128,
}

/// One protocol's facts: its wire number, line and shape hash, and its
/// functions' wire numbers, ascending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocol {
    pub number: u16,
    pub line: Release,
    pub shape_hash: Hash128,
    pub functions: Vec<u16>,
}

/// What an end declares: its parameters' wire part, the package id and
/// version, the encodings, and the protocols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub exchange: Exchange,
    /// `id` in `config.idp`: 32 hex digits, 16 bytes on the wire.
    pub package_id: Hash128,
    pub version: PackageVersion,
    /// Ascending by kind, then hash.
    pub encodings: Vec<Encoding>,
    /// Ascending by number.
    pub protocols: Vec<Protocol>,
}

/// What a [`Hello`] holds whichever protocols it lists: the exchange, the
/// package's id and version, and the connection's encodings. A generator
/// emits it once for the package, beside one [`ProtocolEntry`] per protocol,
/// and [`Hello::build`] puts them together at runtime, so an end lists only
/// the protocols it provides and calls. Every field is a constant a generator
/// can write out, hence the borrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelloHeader<'a> {
    pub exchange: Exchange,
    /// `id` in `config.idp`.
    pub package_id: Hash128,
    /// The package version's release.
    pub version: Release,
    /// The package version's pre-release (`dev.2`), empty for a release.
    pub pre: &'a str,
    /// The encodings the connection uses, in any order: [`Hello::build`]
    /// puts them in the wire's.
    pub encodings: &'a [Encoding],
}

/// One protocol's entry for a [`Hello`]: [`Protocol`], borrowing its function
/// numbers so a generator can write each one out as a constant. It carries no
/// direction: an end lists a protocol it provides and one it calls the same
/// way, and which is which is for the generated code, which knows
/// `@provider`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolEntry<'a> {
    pub number: u16,
    /// The release the protocol last broke at.
    pub line: Release,
    pub shape_hash: Hash128,
    /// The functions' wire numbers, in any order.
    pub functions: &'a [u16],
}

impl ProtocolEntry<'_> {
    fn to_protocol(self) -> Protocol {
        Protocol {
            number: self.number,
            line: self.line,
            shape_hash: self.shape_hash,
            functions: self.functions.to_vec(),
        }
    }
}

impl Hello {
    /// The Hello of one end that provides `provided` and calls `called`: the
    /// header's package and encodings, and exactly those protocols' entries,
    /// in [`canonical`](Self::canonical) order.
    ///
    /// - **Union.** The two sets are listed together, as the wire has one
    ///   list. A protocol in both, or an entry given twice, is listed once
    ///   (the first entry given for a number).
    /// - **Called.** A generator passes every protocol of the other side
    ///   unless the application narrowed what it calls: the peer refuses the
    ///   ones it doesn't have (`unknown_protocol`), and calls to them fail
    ///   locally.
    /// - **Everything.** With every protocol of the package's connection among
    ///   the two, the result is the Hello a generator used to embed whole,
    ///   byte for byte.
    ///
    /// The `exchange` is the header's, and [`Comline2::new`](super::Comline2::new)
    /// sets it from its parameters anyway.
    pub fn build(
        header: &HelloHeader<'_>,
        provided: &[ProtocolEntry<'_>],
        called: &[ProtocolEntry<'_>],
    ) -> Self {
        let mut protocols: Vec<Protocol> = Vec::with_capacity(provided.len() + called.len());
        for entry in provided.iter().chain(called) {
            if !protocols.iter().any(|p| p.number == entry.number) {
                protocols.push(entry.to_protocol());
            }
        }
        Hello {
            exchange: header.exchange,
            package_id: header.package_id,
            version: PackageVersion {
                release: header.version,
                pre: String::from(header.pre),
            },
            encodings: header.encodings.to_vec(),
            protocols,
        }
        .canonical()
    }

    /// Sorts encodings, protocols and functions into the order the wire
    /// requires.
    pub fn canonical(mut self) -> Self {
        self.encodings.sort();
        self.encodings.dedup();
        self.protocols.sort_by_key(|p| p.number);
        for p in &mut self.protocols {
            p.functions.sort_unstable();
            p.functions.dedup();
        }
        self
    }

    pub(super) fn protocol(&self, number: u16) -> Option<&Protocol> {
        self.protocols
            .binary_search_by_key(&number, |p| p.number)
            .ok()
            .map(|i| &self.protocols[i])
    }
}
