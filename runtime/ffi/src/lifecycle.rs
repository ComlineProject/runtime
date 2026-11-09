use core::ptr;
use std::panic::{self, AssertUnwindSafe};

use crate::{
    comline_comline2_config, comline_config, comline_connection, comline_status, guarded, input,
    text, Framing, Handshake2, COMLINE_ERR_INVALID_ARGUMENT, COMLINE_ERR_UNKNOWN_FRAMING,
    COMLINE_EXCHANGE_OFFER, COMLINE_EXCHANGE_SYMMETRIC, COMLINE_HANDSHAKE_COMLINE,
    COMLINE_HANDSHAKE_COMLINE2, COMLINE_HANDSHAKE_NONE, COMLINE_MAX_FRAME_CEILING,
    COMLINE_NEWER_CONSUMER_DEGRADE, COMLINE_NEWER_CONSUMER_REFUSE, COMLINE_OK,
    COMLINE_SAME_VERSION_DIFFERS_REFUSE, COMLINE_SAME_VERSION_DIFFERS_WARN, COMLINE_SCOPE_PACKAGE,
    COMLINE_SCOPE_PROTOCOL,
};
use comline_runtime::connection::State;
use comline_runtime::contract::stream::DEFAULT_MAX_FRAME;
use comline_runtime::contract::{
    HandshakeFrame, Side, FRAMING_DATAGRAM2, FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};
use comline_runtime::handshake::comline2::{
    Comline2, Exchange, Message, NewerConsumer, Params, SameVersionDiffers, Scope,
};

/// Make a connection from `cfg`, into `*out`. The one allocation the library
/// makes: the handle, with room for `max_waiting` waiting calls. (One known
/// exception, a bug: `comline_connection_feed` on `jsonrpc@1`; see there.)
///
/// Errors: `COMLINE_ERR_INVALID_ARGUMENT` (a null pointer, a name that isn't
/// UTF-8, a field outside its values, `max_frame` above the ceiling,
/// `*out` overlapping the config or its names) or
/// `COMLINE_ERR_UNKNOWN_FRAMING`. `*out` is null on any error, unless it
/// overlaps an input, when it is left as it was.
///
/// # Safety
/// `cfg` is null or a valid `comline_config` whose name pointers satisfy
/// their lengths; `out` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_new(
    cfg: *const comline_config,
    out: *mut *mut comline_connection,
) -> comline_status {
    // SAFETY: the caller's contract.
    unsafe { connection_new(cfg, ptr::null(), out) }
}

/// [`comline_connection_new`] for a `comline@2` connection: `cfg->handshake`
/// must be `COMLINE_HANDSHAKE_COMLINE2`, and `comline2` holds its facts and
/// parameters. `cfg->ir_hash` and `cfg->capabilities` aren't read: `comline@2`
/// carries neither.
///
/// Errors: as `comline_connection_new`, and `COMLINE_ERR_INVALID_ARGUMENT` for
/// a `hello` that isn't an encoded hello message or a field outside its values.
///
/// # Safety
/// As `comline_connection_new`; `comline2` is null or a valid
/// `comline_comline2_config` whose `hello` satisfies `hello_len`.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_new_comline2(
    cfg: *const comline_config,
    comline2: *const comline_comline2_config,
    out: *mut *mut comline_connection,
) -> comline_status {
    if comline2.is_null() {
        if !out.is_null() {
            // SAFETY: checked non-null; the caller's contract.
            unsafe { out.write(ptr::null_mut()) };
        }
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe { connection_new(cfg, comline2, out) }
}

/// Makes a connection, with `comline2` for a `comline@2` one (else null).
///
/// # Safety
/// As `comline_connection_new_comline2`.
unsafe fn connection_new(
    cfg: *const comline_config,
    comline2: *const comline_comline2_config,
    out: *mut *mut comline_connection,
) -> comline_status {
    guarded(|| {
        if out.is_null() {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        if cfg.is_null() {
            // SAFETY: checked non-null; the caller's contract.
            unsafe { out.write(ptr::null_mut()) };
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        if !disjoint!(writes[(out, 1)], reads[(cfg, 1), (comline2, 1)]) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null and apart; the caller's contract.
        let cfg = unsafe { &*cfg };
        if !disjoint!(
            writes [(out, 1)],
            reads [
                (cfg.framing, cfg.framing_len),
                (cfg.wire_format, cfg.wire_format_len),
            ]
        ) {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null and apart; the caller's contract.
        unsafe { out.write(ptr::null_mut()) };
        // SAFETY: the caller's contract.
        let (Some(framing_name), Some(wire_format)) = (unsafe {
            (
                text(cfg.framing, cfg.framing_len),
                text(cfg.wire_format, cfg.wire_format_len),
            )
        }) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        let flag = |v: u8| match v {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        let (Some(stream), Some(unordered)) = (flag(cfg.stream), flag(cfg.unordered)) else {
            return COMLINE_ERR_INVALID_ARGUMENT;
        };
        if cfg._reserved != 0 || cfg.max_frame > COMLINE_MAX_FRAME_CEILING {
            return COMLINE_ERR_INVALID_ARGUMENT;
        }
        let framing = match framing_name {
            FRAMING_DATAGRAM2 => Framing::Datagram2,
            FRAMING_DATAGRAM3 => Framing::Datagram3,
            FRAMING_JSONRPC => Framing::JsonRpc,
            _ => return COMLINE_ERR_UNKNOWN_FRAMING,
        };
        let ours = match cfg.handshake {
            COMLINE_HANDSHAKE_COMLINE if comline2.is_null() => Some(HandshakeFrame::new(
                cfg.ir_hash,
                wire_format,
                framing_name,
                cfg.capabilities,
            )),
            COMLINE_HANDSHAKE_NONE if comline2.is_null() => None,
            COMLINE_HANDSHAKE_COMLINE2 if !comline2.is_null() => None,
            _ => return COMLINE_ERR_INVALID_ARGUMENT,
        };
        let comline2 = if comline2.is_null() {
            None
        } else {
            // SAFETY: checked non-null; the caller's contract.
            let c = unsafe { &*comline2 };
            if !disjoint!(writes[(out, 1)], reads[(c.hello, c.hello_len)]) {
                return COMLINE_ERR_INVALID_ARGUMENT;
            }
            // SAFETY: the caller's contract.
            let Some(hello) = (unsafe { input(c.hello, c.hello_len) }) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            let Some(handshake) = handshake2(c, hello) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            Some(Box::new(handshake))
        };
        let settled = ours.is_none() && comline2.is_none();
        let max_frame = match cfg.max_frame {
            0 => DEFAULT_MAX_FRAME,
            // At most the ceiling, so it fits a `usize` on every target.
            n => n as usize,
        };
        let capacity = cfg.max_waiting as usize;
        let state = if unordered {
            State::unordered_with_capacity(capacity)
        } else {
            State::with_capacity(capacity)
        };
        let conn = Box::new(comline_connection {
            state,
            framing,
            ours,
            comline2,
            sent: settled,
            received: settled,
            closed: false,
            poisoned: false,
            stream,
            max_frame,
            #[cfg(feature = "test-hooks")]
            panic_next: false,
        });
        // SAFETY: checked non-null.
        unsafe { out.write(Box::into_raw(conn)) };
        COMLINE_OK
    })
}

/// Free a connection. Null is a no-op. Works on a poisoned or closed handle.
///
/// # Safety
/// `conn` is null or a handle from `comline_connection_new`, not freed
/// before, with no other call on it running. It is invalid afterwards.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_free(conn: *mut comline_connection) {
    // Not through `with_conn`, which refuses a poisoned or closed handle:
    // those are exactly the ones a binding frees. Freeing one is sound. A
    // panic unwinds out of safe code only, which leaves every field (plain
    // integers, flags, a `Vec<u64>`) valid, so it drops like any other.
    if conn.is_null() {
        return;
    }
    // Dropping holds no user code; a panic here would be a bug in `Vec`.
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller's contract.
        drop(unsafe { Box::from_raw(conn) });
    }));
}

/// A [`Handshake2`] from its config, or `None` for a field outside its values.
fn handshake2(c: &comline_comline2_config, hello: &[u8]) -> Option<Handshake2> {
    if c._reserved != [0; 3] {
        return None;
    }
    let Ok(Message::Hello(hello)) = Message::decode(hello) else {
        return None;
    };
    let params = Params {
        scope: match c.scope {
            COMLINE_SCOPE_PROTOCOL => Scope::Protocol,
            COMLINE_SCOPE_PACKAGE => Scope::Package,
            _ => return None,
        },
        newer_consumer: match c.newer_consumer {
            COMLINE_NEWER_CONSUMER_DEGRADE => NewerConsumer::Degrade,
            COMLINE_NEWER_CONSUMER_REFUSE => NewerConsumer::Refuse,
            _ => return None,
        },
        same_version_differs: match c.same_version_differs {
            COMLINE_SAME_VERSION_DIFFERS_REFUSE => SameVersionDiffers::Refuse,
            COMLINE_SAME_VERSION_DIFFERS_WARN => SameVersionDiffers::Warn,
            _ => return None,
        },
        exchange: match c.exchange {
            COMLINE_EXCHANGE_OFFER => Exchange::Offer,
            COMLINE_EXCHANGE_SYMMETRIC => Exchange::Symmetric,
            _ => return None,
        },
    };
    Some(Handshake2::new(
        Comline2::new(params, hello),
        Side::from_code(c.side)?,
    ))
}
