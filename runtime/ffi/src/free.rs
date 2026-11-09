use std::panic::{self};

use crate::{
    comline_handshake, comline_status, guarded, input, output, status_of, text,
    COMLINE_BUFFER_TOO_SMALL, COMLINE_ERR_INVALID_ARGUMENT, COMLINE_HANDSHAKE_LEN, COMLINE_OK,
};
use comline_runtime::contract::{
    name_hash, HandshakeFrame, MethodName, SliceBuf, FRAMING_DATAGRAM2, FRAMING_DATAGRAM3,
    FRAMING_JSONRPC, HANDSHAKE_COMLINE, HANDSHAKE_COMLINE2, HANDSHAKE_NONE,
};

/// The FNV-1a-64 hash of an encoding name (UTF-8), as the handshake carries
/// it, into `*hash`.
///
/// # Safety
/// `name` is null or readable for `len`; `hash` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_name_hash(
    name: *const u8,
    len: usize,
    hash: *mut u64,
) -> comline_status {
    guarded(|| {
        if !disjoint!(writes[(hash, 1)], reads[(name, len)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(name), Some(hash)) = (unsafe { text(name, len) }, unsafe { hash.as_mut() })
        else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        *hash = name_hash(name);
        COMLINE_OK
    })
}

/// Write a `comline@1` handshake frame (31 bytes, no length prefix), for a
/// host that runs the handshake itself. The names are UTF-8 and any name
/// is taken, `datagram@1` included; they are hashed into the frame.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` = 31),
/// `COMLINE_ERR_INVALID_ARGUMENT`.
///
/// # Safety
/// The name pointers are null or readable for their lengths; `out` is null
/// or `cap` writable bytes; `written` is null or writable.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn comline_handshake_encode(
    ir_hash: u64,
    wire_format: *const u8,
    wire_format_len: usize,
    framing: *const u8,
    framing_len: usize,
    capabilities: u32,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    guarded(|| {
        if !disjoint!(
            writes [(out, cap), (written, 1)],
            reads [(wire_format, wire_format_len), (framing, framing_len)]
        ) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(wire_format), Some(framing), Some(out), Some(written)) = (unsafe {
            (
                text(wire_format, wire_format_len),
                text(framing, framing_len),
                output(out, cap),
                written.as_mut(),
            )
        }) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let frame = HandshakeFrame::new(ir_hash, wire_format, framing, capabilities);
        let mut buf = SliceBuf::new(out);
        frame.encode(&mut buf);
        if buf.overflowed() {
            *written = COMLINE_HANDSHAKE_LEN as usize;
            return COMLINE_BUFFER_TOO_SMALL;
        }
        *written = buf.written().len();
        COMLINE_OK
    })
}

/// Check a peer's `comline@1` handshake frame against ours, for a host that
/// runs the handshake itself. `ours` is our frame as
/// `comline_handshake_encode` wrote it. `COMLINE_OK` when they agree on
/// schema, wire format and framing (capability bits may differ);
/// `COMLINE_ERR_HANDSHAKE` when they don't, or `peer` doesn't parse. As in
/// the core, bytes after the 31-byte frame are ignored (runtime#33).
///
/// `*peer_out`, unless null, gets the peer's frame whenever it parses,
/// agreeing or not, so a host can say what the peer declared.
///
/// Errors: `COMLINE_ERR_INVALID_ARGUMENT` when `ours` doesn't parse.
///
/// # Safety
/// `ours` and `peer` are null or readable for their lengths; `peer_out` is
/// null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_handshake_check(
    ours: *const u8,
    ours_len: usize,
    peer: *const u8,
    peer_len: usize,
    peer_out: *mut comline_handshake,
) -> comline_status {
    guarded(|| {
        // `peer_out` is the only memory written, so only when it's given can
        // anything overlap.
        if !peer_out.is_null()
            && !disjoint!(
                writes [(peer_out, 1)],
                reads [(ours, ours_len), (peer, peer_len)]
            )
        {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(ours), Some(peer)) = (unsafe { input(ours, ours_len) }, unsafe {
            input(peer, peer_len)
        }) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let Some(ours) = HandshakeFrame::decode(ours) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked apart from the inputs; the caller's contract.
        if let Some(out) = unsafe { peer_out.as_mut() } {
            if let Some(frame) = HandshakeFrame::decode(peer) {
                *out = comline_handshake {
                    ir_hash: frame.ir_hash,
                    wire_format: frame.wire_format,
                    framing: frame.framing,
                    capabilities: frame.capabilities,
                    _reserved: 0,
                };
            }
        }
        match ours.check_bytes(peer) {
            Ok(_) => COMLINE_OK,
            Err(error) => status_of(error),
        }
    })
}

/// 1 when this library carries the named framing or handshake (UTF-8) on a
/// connection, 0 otherwise: `datagram@2`, `datagram@3`, `jsonrpc@1`,
/// `comline@1`, `comline@2`, `none`.
/// A binding checks a schema's `@framing` here at setup, not at the first
/// frame. Wire formats are the host's codecs, so none is listed; the
/// lockstep `datagram@1` has no connection form and is 0.
///
/// # Safety
/// `name` is null or readable for `len`.
#[no_mangle]
pub unsafe extern "C" fn comline_supports(name: *const u8, len: usize) -> u8 {
    panic::catch_unwind(|| {
        // SAFETY: the caller's contract.
        let name = unsafe { text(name, len) };
        u8::from(matches!(
            name,
            Some(
                FRAMING_DATAGRAM2
                    | FRAMING_DATAGRAM3
                    | FRAMING_JSONRPC
                    | HANDSHAKE_COMLINE
                    | HANDSHAKE_COMLINE2
                    | HANDSHAKE_NONE
            )
        ))
    })
    .unwrap_or(0)
}

/// Unescape a method name as a frame spelled it (a `call_name` span with
/// `COMLINE_EVENT_NAME_ESCAPED`: the JSON string's text between its quotes)
/// into UTF-8 in `out`. Text with no escapes is copied as it is.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is the exact size needed,
/// and nothing was written), `COMLINE_ERR_INVALID_ARGUMENT` when `in` isn't
/// a valid JSON string's text.
///
/// # Safety
/// `in` is null or readable for `len`; `out` is null or `cap` writable
/// bytes; `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_method_name_unescape(
    in_: *const u8,
    len: usize,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    guarded(|| {
        if !disjoint!(writes [(out, cap), (written, 1)], reads [(in_, len)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller's contract.
        let (Some(spelled), Some(out), Some(written)) = (
            unsafe { text(in_, len) },
            unsafe { output(out, cap) },
            unsafe { written.as_mut() },
        ) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        *written = 0;
        let Some(name) = MethodName::from_json(spelled) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let needed: usize = name.chars().map(char::len_utf8).sum();
        if needed > out.len() {
            *written = needed;
            return COMLINE_BUFFER_TOO_SMALL;
        }
        let mut at = 0;
        for c in name.chars() {
            at += c.encode_utf8(&mut out[at..]).len();
        }
        *written = at;
        COMLINE_OK
    })
}
