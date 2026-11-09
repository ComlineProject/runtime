use core::ptr;

use crate::{
    comline_connection, comline_protocol_outcome, comline_refusal, comline_status, comline_version,
    guarded, input, status_of, with_closed_too, with_conn, COMLINE_BUFFER_TOO_SMALL,
    COMLINE_COMLINE2_ANSWER, COMLINE_COMLINE2_HELLO, COMLINE_COMLINE2_VERDICT,
    COMLINE_ERR_HANDSHAKE, COMLINE_ERR_INVALID_ARGUMENT, COMLINE_ERR_STATE, COMLINE_OK,
};
use comline_runtime::handshake::comline2::{Message, Negotiated, ProtocolState};

/// Writes `items` to the `cap` values at `out`: `*written` is how many there
/// are, and `COMLINE_BUFFER_TOO_SMALL` when they don't fit, with nothing
/// written.
///
/// # Safety
/// `out` is null or `cap` writable, aligned values of `T`; `written` is null
/// or writable; neither overlaps `items`.
unsafe fn list_out<T: Copy>(
    items: &[T],
    out: *mut T,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    // SAFETY: the caller's contract.
    let Some(written) = (unsafe { written.as_mut() }) else {
        return COMLINE_ERR_INVALID_ARGUMENT;
    };
    if cap.saturating_mul(core::mem::size_of::<T>()) > isize::MAX as usize
        || (out.is_null() && cap != 0)
    {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    *written = items.len();
    if items.len() > cap {
        return COMLINE_BUFFER_TOO_SMALL;
    }
    if !items.is_empty() {
        // SAFETY: `items.len() <= cap`, which the caller's contract makes room
        // for, so `out` isn't null, and it doesn't overlap `items`.
        unsafe { ptr::copy_nonoverlapping(items.as_ptr(), out, items.len()) };
    }
    COMLINE_OK
}

/// What the handshake has concluded, for a function that reads it.
pub(crate) fn negotiated(conn: &comline_connection) -> Option<&Negotiated> {
    conn.comline2.as_ref()?.negotiated.as_ref()
}

/// Whether this end may call `function` of `protocol`, both wire numbers:
/// `COMLINE_OK` to send it, or `COMLINE_ERR_UNAVAILABLE` when the peer lacks the
/// function or the protocol was refused. Call it before encoding a request,
/// so a call the peer can't take never reaches the wire.
///
/// On a connection without `comline@2` it's always `COMLINE_OK`: there nothing
/// is negotiated per function. Before the handshake concluded, or after it
/// refused, it's `COMLINE_ERR_STATE`.
///
/// # Safety
/// `conn` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_check_call(
    conn: *mut comline_connection,
    protocol: u16,
    function: u16,
) -> comline_status {
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            if conn.comline2.is_none() {
                return COMLINE_OK;
            }
            match negotiated(conn) {
                None => COMLINE_ERR_STATE,
                Some(n) => match n.check_call(protocol, function) {
                    Ok(()) => COMLINE_OK,
                    Err(error) => status_of(error),
                },
            }
        })
    }
}

/// Why `comline@2` refused this connection, into `*refusal`, after
/// `COMLINE_ERR_REFUSED`. It works on the closed handle a refusal leaves.
///
/// Errors: `COMLINE_ERR_STATE` when nothing was refused.
///
/// # Safety
/// `conn` is null or a live handle; `refusal` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_refusal(
    conn: *mut comline_connection,
    refusal: *mut comline_refusal,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (refusal, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_closed_too(conn, |conn| {
            let Some(refusal) = refusal.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *refusal = comline_refusal::default();
            let Some(r) = conn.comline2.as_ref().and_then(|h| h.refusal) else {
                return COMLINE_ERR_STATE;
            };
            refusal.reason = r.reason.code();
            refusal.side = r.side.code();
            if let Some(protocol) = r.protocol {
                refusal.has_protocol = 1;
                refusal.protocol = protocol;
            }
            COMLINE_OK
        })
    }
}

/// The wire numbers of the protocols that connected although a shape differs
/// at the same package version (`same_version_differs = warn`), ascending, to
/// report to whoever runs this: one end isn't what its version claims. `cap`
/// and `*written` count numbers.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is how many there are),
/// `COMLINE_ERR_STATE` before the handshake concluded, after it refused, or
/// without `comline@2`.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable numbers;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_warnings(
    conn: *mut comline_connection,
    out: *mut u16,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| match negotiated(conn) {
            None => COMLINE_ERR_STATE,
            // SAFETY: the caller's contract.
            Some(n) => list_out(&n.warnings, out, cap, written),
        })
    }
}

/// What each of this end's protocols came to, ascending by number. `cap` and
/// `*written` count outcomes. It holds what this end concluded, and the peer's
/// verdict can still refuse a protocol until `COMLINE_EVENT_HANDSHAKE_OK`.
///
/// Errors: as `comline_connection_warnings`.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable outcomes;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_protocols(
    conn: *mut comline_connection,
    out: *mut comline_protocol_outcome,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(n) = negotiated(conn) else {
                return COMLINE_ERR_STATE;
            };
            let Some(written) = written.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            let size = cap.saturating_mul(core::mem::size_of::<comline_protocol_outcome>());
            if size > isize::MAX as usize || (out.is_null() && cap != 0) {
                return COMLINE_ERR_INVALID_ARGUMENT;
            }
            *written = n.protocols.len();
            if n.protocols.len() > cap {
                return COMLINE_BUFFER_TOO_SMALL;
            }
            for (i, p) in n.protocols.iter().enumerate() {
                let reason = match &p.state {
                    ProtocolState::Accepted { .. } => 0,
                    ProtocolState::Refused { reason } => reason.code(),
                };
                let outcome = comline_protocol_outcome {
                    number: p.number,
                    reason,
                    _reserved: 0,
                };
                // SAFETY: `i < cap`, which the caller's contract makes room for.
                out.add(i).write(outcome);
            }
            COMLINE_OK
        })
    }
}

/// The functions of `protocol` the peer lacks, ascending by number: those
/// `comline_connection_check_call` refuses. `cap` and `*written` count numbers.
/// A protocol this end doesn't have, or one refused, lacks none.
///
/// Errors: as `comline_connection_warnings`.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable numbers;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_peer_lacks(
    conn: *mut comline_connection,
    protocol: u16,
    out: *mut u16,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(n) = negotiated(conn) else {
                return COMLINE_ERR_STATE;
            };
            let lacks = n
                .protocols
                .iter()
                .find(|p| p.number == protocol)
                .and_then(|p| match &p.state {
                    ProtocolState::Accepted { peer_lacks } => Some(peer_lacks.as_slice()),
                    ProtocolState::Refused { .. } => None,
                });
            // SAFETY: the caller's contract.
            list_out(lacks.unwrap_or(&[]), out, cap, written)
        })
    }
}

/// The peer's package version: its release in `*version`, and its pre-release
/// (`dev.2`, empty for a release) as UTF-8 in `pre`. `cap` and `*written` count
/// the pre-release's bytes.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is the size needed, and
/// `*version` is set), and as `comline_connection_warnings`.
///
/// # Safety
/// `conn` is null or a live handle; `version` and `written` are null or
/// writable; `pre` is null or `cap` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_peer_version(
    conn: *mut comline_connection,
    version: *mut comline_version,
    pre: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (version, 1), (pre, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(version) = version.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *version = comline_version::default();
            let Some(n) = negotiated(conn) else {
                return COMLINE_ERR_STATE;
            };
            let release = n.peer_version.release;
            *version = comline_version {
                major: release.major,
                minor: release.minor,
                patch: release.patch,
            };
            // SAFETY: the caller's contract.
            list_out(n.peer_version.pre.as_bytes(), pre, cap, written)
        })
    }
}

/// Read one `comline@2` message: `COMLINE_OK` and its type in `*kind`
/// (`COMLINE_COMLINE2_*`), or `COMLINE_ERR_HANDSHAKE` and why it can't be read
/// in `*reason` (`COMLINE_REASON_MALFORMED`, or
/// `COMLINE_REASON_HANDSHAKE_VERSION` for another version's frame). The other
/// is 0. For a tool or a test: a connection reads its messages itself.
///
/// # Safety
/// `in` is null or readable for `len`; `kind` and `reason` are null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_comline2_message_check(
    in_: *const u8,
    len: usize,
    kind: *mut u8,
    reason: *mut u8,
) -> comline_status {
    guarded(|| {
        if !disjoint!(writes [(kind, 1), (reason, 1)], reads [(in_, len)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked apart; the caller's contract.
        let (Some(bytes), Some(kind), Some(reason)) =
            (unsafe { (input(in_, len), kind.as_mut(), reason.as_mut()) })
        else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        (*kind, *reason) = (0, 0);
        match Message::decode(bytes) {
            Ok(message) => {
                *kind = match message {
                    Message::Hello(_) => COMLINE_COMLINE2_HELLO,
                    Message::Answer(_) => COMLINE_COMLINE2_ANSWER,
                    Message::Verdict(_) => COMLINE_COMLINE2_VERDICT,
                };
                COMLINE_OK
            }
            Err(why) => {
                *reason = why.code();
                COMLINE_ERR_HANDSHAKE
            }
        }
    })
}
