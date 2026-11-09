// Encoding names are versioned, `name@major`: a later `msgpack@2` must never
// be mistaken for today's positional `msgpack@1`. The handshake hashes them,
// so peers on different versions of an encoding refuse each other.

/// Name of the Comline datagram framing (`wire::encode_request` /
/// `encode_response`). Pass it to [`HandshakeFrame::new`](crate::contract::HandshakeFrame::new).
pub const FRAMING_DATAGRAM: &str = "datagram@1";
/// Name of the Comline datagram framing with a request/reply kind and a
/// protocol number (`contract::Datagram2Framing`), for a
/// [`Connection`](crate::connection::Connection).
pub const FRAMING_DATAGRAM2: &str = "datagram@2";
/// Name of the datagram framing that addresses a call by its protocol's and
/// function's wire numbers (`contract::Datagram3Framing`, `calls = by_id`).
pub const FRAMING_DATAGRAM3: &str = "datagram@3";

/// Name of the JSON-RPC 2.0 framing (`framing::JsonRpcFraming`).
pub const FRAMING_JSONRPC: &str = "jsonrpc@1";
/// Name of the JSON wire format (`format::Json`).
pub const FORMAT_JSON: &str = "json@1";
/// Name of the MessagePack wire format (`format::MsgPack`).
pub const FORMAT_MSGPACK: &str = "msgpack@1";
/// Name of the packed wire format (`format::Packed`): fixed-width fields back
/// to back, varint lengths. The default a package gets.
pub const FORMAT_PACKED: &str = "packed@1";

/// Name of Comline's own handshake, the fixed [`HandshakeFrame`](crate::contract::HandshakeFrame) each end sends
/// (`handshake::ComlineHandshake`).
pub const HANDSHAKE_COMLINE: &str = "comline@1";
/// Name of the compatibility-checking handshake (`handshake::comline2::Comline2Handshake`,
/// runtime#15): lines, shape hashes and wire numbers instead of an exact IR
/// match.
pub const HANDSHAKE_COMLINE2: &str = "comline@2";
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_hash_is_deterministic_and_name_specific() {
        assert_eq!(name_hash("msgpack"), name_hash("msgpack"));
        assert_ne!(name_hash("msgpack"), name_hash("json"));
        assert_ne!(name_hash("msgpack"), name_hash("com.acme.msgpack"));
    }
}
