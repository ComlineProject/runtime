use crate::contract::{name_hash, BufMut, RuntimeError};

const MAGIC: [u8; 2] = *b"CO";
const VERSION: u8 = 1;
/// `[MAGIC:2][VERSION:1][ir_hash:u64 LE][wire_format:u64 LE][framing:u64 LE][capabilities:u32 LE]`
const LEN: usize = 2 + 1 + 8 + 8 + 8 + 4;

/// The frame of the `comline@1` handshake ([`HANDSHAKE_COMLINE`](crate::contract::HANDSHAKE_COMLINE)): what each
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
    /// [`name_hash`] of the framing name ([`FRAMING_DATAGRAM`](crate::contract::FRAMING_DATAGRAM), …).
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

    /// The sans-IO handshake step: parse the peer's frame from `peer` and
    /// [`check`](Self::check) it against ours. Returns the peer's frame when
    /// they agree; `Err(RuntimeError::Handshake)` if it doesn't parse, or
    /// disagrees.
    ///
    /// As [`decode`](Self::decode), bytes after the frame are ignored
    /// (runtime#33).
    pub fn check_bytes(&self, peer: &[u8]) -> Result<HandshakeFrame, RuntimeError> {
        let peer = Self::decode(peer).ok_or(RuntimeError::Handshake)?;
        self.check(&peer)?;
        Ok(peer)
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use crate::contract::{FORMAT_JSON, FORMAT_MSGPACK, FRAMING_DATAGRAM};
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

    fn bytes(frame: &HandshakeFrame) -> Vec<u8> {
        let mut out = Vec::new();
        frame.encode(&mut out);
        out
    }

    #[test]
    fn check_bytes_decodes_and_checks() {
        let ours = sample();
        assert_eq!(ours.check_bytes(&bytes(&sample())), Ok(sample()));

        let mut caps_differ = sample();
        caps_differ.capabilities = 0;
        assert_eq!(
            ours.check_bytes(&bytes(&caps_differ)),
            Ok(caps_differ),
            "capability bits may differ, and the peer's are returned"
        );

        let mut schema_differ = sample();
        schema_differ.ir_hash = 0;
        assert_eq!(
            ours.check_bytes(&bytes(&schema_differ)),
            Err(RuntimeError::Handshake)
        );

        let whole = bytes(&sample());
        assert_eq!(ours.check_bytes(&[]), Err(RuntimeError::Handshake));
        assert_eq!(
            ours.check_bytes(&whole[..LEN - 1]),
            Err(RuntimeError::Handshake)
        );
        let mut bad_magic = whole.clone();
        bad_magic[0] = b'X';
        assert_eq!(ours.check_bytes(&bad_magic), Err(RuntimeError::Handshake));
    }

    /// Pins current behaviour, pending runtime#33 (undecided): bytes after the
    /// 31-byte frame are ignored, by `decode` and so by `check_bytes`. If
    /// runtime#33 decides to refuse them, this test changes with it.
    #[test]
    fn trailing_bytes_are_ignored_pending_runtime_33() {
        let mut frame = bytes(&sample());
        frame.extend_from_slice(b"trailing");
        assert_eq!(HandshakeFrame::decode(&frame), Some(sample()));
        assert_eq!(sample().check_bytes(&frame), Ok(sample()));
    }
}
