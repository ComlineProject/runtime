use crate::contract::{BufMut, RuntimeError};

const MAGIC: [u8; 2] = *b"CO";
const VERSION: u8 = 1;
/// `[MAGIC:2][VERSION:1][ir_hash:u64 LE][wire_format:u64 LE][framing:u64 LE][capabilities:u32 LE]`
const LEN: usize = 2 + 1 + 8 + 8 + 8 + 4;

// Encoding names are versioned, `name@major`: a later `msgpack@2` must never
// be mistaken for today's positional `msgpack@1`. The handshake hashes them,
// so peers on different versions of an encoding refuse each other.

/// Name of the Comline datagram framing (`wire::encode_request` /
/// `encode_response`). Pass it to [`HandshakeFrame::new`].
pub const FRAMING_DATAGRAM: &str = "datagram@1";
/// Name of the JSON-RPC 2.0 framing (`framing::JsonRpcFraming`).
pub const FRAMING_JSONRPC: &str = "jsonrpc@1";
/// Name of the JSON wire format (`format::Json`).
pub const FORMAT_JSON: &str = "json@1";
/// Name of the MessagePack wire format (`format::MsgPack`).
pub const FORMAT_MSGPACK: &str = "msgpack@1";

/// Name of Comline's own handshake, the fixed [`HandshakeFrame`] each end sends
/// (`handshake::ComlineHandshake`).
pub const HANDSHAKE_COMLINE: &str = "comline@1";
/// Name of the official "no handshake" choice (`handshake::NoHandshake`): no
/// frame, nothing checked. Unversioned, since there is nothing to evolve.
pub const HANDSHAKE_NONE: &str = "none";

/// 64-bit FNV-1a. Folds a wire-format / framing **name** into the fixed-size
/// handshake without a central id registry — a user's add-on format picks a
/// namespaced name (`"com.acme.myformat@1"`) and its hash won't collide with a
/// built-in. Stable across platforms.
pub fn name_hash(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The frame of the `comline@1` handshake ([`HANDSHAKE_COMLINE`]): what each
/// end declares when a connection opens — the schema fingerprint the generated
/// code was built from, the serialization + framing it speaks, and its
/// transport capability bits.
///
/// `handshake::ComlineHandshake` exchanges these and refuses on a mismatch —
/// catching "one end MessagePack, one end JSON" or two ends built from
/// different schema versions. Other handshakes (`none`, a custom one) send
/// something else, or nothing.
///
/// Lifetime-free and `Copy`: the format / framing are carried as
/// [`name_hash`]es of their names, not the strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeFrame {
    /// Fingerprint of the frozen IR the two ends generated from. A generator
    /// emits this as a constant.
    pub ir_hash: u64,
    /// [`name_hash`] of the [`WireFormat::name`](crate::contract::WireFormat::name).
    pub wire_format: u64,
    /// [`name_hash`] of the framing name ([`FRAMING_DATAGRAM`], …).
    pub framing: u64,
    /// Transport capability bits (reliable / ordered / duplex / …). Advisory:
    /// a difference here is not a mismatch.
    pub capabilities: u32,
}

impl HandshakeFrame {
    /// Build one from the format / framing **names** (hashed in).
    pub fn new(ir_hash: u64, wire_format: &str, framing: &str, capabilities: u32) -> Self {
        Self {
            ir_hash,
            wire_format: name_hash(wire_format),
            framing: name_hash(framing),
            capabilities,
        }
    }

    /// Write the fixed-size handshake frame.
    pub fn encode(&self, out: &mut dyn BufMut) {
        out.put_slice(&MAGIC);
        out.put_u8(VERSION);
        out.put_u64_le(self.ir_hash);
        out.put_u64_le(self.wire_format);
        out.put_u64_le(self.framing);
        out.put_u32_le(self.capabilities);
    }

    /// Parse one. `None` if it is truncated, or the magic / version is wrong
    /// (a peer that never sent a handshake, or a different protocol).
    pub fn decode(frame: &[u8]) -> Option<Self> {
        let frame: [u8; LEN] = frame.get(..LEN)?.try_into().ok()?;
        if frame[0] != MAGIC[0] || frame[1] != MAGIC[1] || frame[2] != VERSION {
            return None;
        }
        Some(Self {
            ir_hash: u64::from_le_bytes(frame[3..11].try_into().unwrap()),
            wire_format: u64::from_le_bytes(frame[11..19].try_into().unwrap()),
            framing: u64::from_le_bytes(frame[19..27].try_into().unwrap()),
            capabilities: u32::from_le_bytes(frame[27..31].try_into().unwrap()),
        })
    }

    /// Check a peer's declaration against ours. `Err(RuntimeError::Handshake)`
    /// if `ir_hash`, `wire_format`, or `framing` disagree; capability bits are
    /// allowed to differ.
    pub fn check(&self, peer: &HandshakeFrame) -> Result<(), RuntimeError> {
        let agree = self.ir_hash == peer.ir_hash
            && self.wire_format == peer.wire_format
            && self.framing == peer.framing;
        if agree {
            Ok(())
        } else {
            Err(RuntimeError::Handshake)
        }
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn sample() -> HandshakeFrame {
        HandshakeFrame::new(
            0xdead_beef_0102_0304,
            FORMAT_MSGPACK,
            FRAMING_DATAGRAM,
            0b101,
        )
    }

    #[test]
    fn round_trips() {
        let mut frame = Vec::new();
        sample().encode(&mut frame);
        assert_eq!(frame.len(), LEN);
        assert_eq!(HandshakeFrame::decode(&frame), Some(sample()));
    }

    #[test]
    fn name_hash_is_deterministic_and_name_specific() {
        assert_eq!(name_hash("msgpack"), name_hash("msgpack"));
        assert_ne!(name_hash("msgpack"), name_hash("json"));
        assert_ne!(name_hash("msgpack"), name_hash("com.acme.msgpack"));
    }

    #[test]
    fn rejects_truncated_or_foreign() {
        assert_eq!(HandshakeFrame::decode(&[]), None);
        assert_eq!(HandshakeFrame::decode(&[0u8; LEN]), None); // bad magic
        let mut frame = Vec::new();
        sample().encode(&mut frame);
        frame.truncate(LEN - 1);
        assert_eq!(HandshakeFrame::decode(&frame), None);
    }

    #[test]
    fn check_agrees_and_disagrees() {
        let ours = sample();
        assert!(ours.check(&sample()).is_ok());

        let mut caps_differ = sample();
        caps_differ.capabilities = 0;
        assert!(
            ours.check(&caps_differ).is_ok(),
            "capability bits may differ"
        );

        let fmt_differ = HandshakeFrame::new(
            ours.ir_hash,
            FORMAT_JSON,
            FRAMING_DATAGRAM,
            ours.capabilities,
        );
        assert_eq!(ours.check(&fmt_differ), Err(RuntimeError::Handshake));

        let mut schema_differ = sample();
        schema_differ.ir_hash = 0;
        assert_eq!(ours.check(&schema_differ), Err(RuntimeError::Handshake));
    }
}
