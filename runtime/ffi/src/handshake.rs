use crate::{
    comline_connection, comline_event, comline_span, comline_status, output, with_conn, Awaiting,
    COMLINE_ERR_INVALID_ARGUMENT, COMLINE_ERR_STATE, COMLINE_HANDSHAKE_LEN, COMLINE_OK,
};
use comline_runtime::contract::SliceBuf;

/// Write this side's handshake frame (with its length prefix in stream
/// mode), to send before anything else. Without a handshake it writes
/// nothing and returns `COMLINE_OK`.
///
/// The peer's frame is the first one fed (`COMLINE_EVENT_HANDSHAKE_OK`), and
/// may arrive before or after this call. Requests and replies need both.
///
/// **`comline@2`** has several messages, so call this again whenever an event
/// or a refusal says `COMLINE_EVENT_SEND`, and once at the start. It writes the
/// next message, or nothing (`*written` 0, `COMLINE_OK`) when there's none: under
/// `exchange = offer` the responder has nothing to send until it has read the
/// hello. A second call is therefore no `COMLINE_ERR_STATE` here. Requests and
/// replies need every message written and the handshake finished.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` = 31, or 35 in stream
/// mode; for `comline@2`, the message's size), `COMLINE_ERR_FRAME_TOO_LARGE`
/// (`max_frame` below 31, or below that message),
/// `COMLINE_ERR_STATE` when it was already written.
///
/// # Safety
/// `conn` is null or a live handle; `out` is null or `cap` writable bytes;
/// `written` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_start(
    conn: *mut comline_connection,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (out, cap), (written, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(out), Some(written)) = (output(out, cap), written.as_mut()) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *written = 0;
            if conn.comline2.is_some() {
                return start_comline2(conn, out, written);
            }
            let Some(ours) = conn.ours else {
                return COMLINE_OK;
            };
            if conn.sent {
                return COMLINE_ERR_STATE;
            }
            let len = COMLINE_HANDSHAKE_LEN as usize;
            let (head, room) = conn.split_out(out);
            let mut buf = SliceBuf::new(room);
            ours.encode(&mut buf);
            if buf.overflowed() {
                return conn.too_small(len, written);
            }
            *written = conn.finish(head, len);
            conn.sent = true;
            COMLINE_OK
        })
    }
}

/// [`comline_connection_start`] on a `comline@2` connection.
#[cold]
#[inline(never)]
fn start_comline2(
    conn: &mut comline_connection,
    out: &mut [u8],
    written: &mut usize,
) -> comline_status {
    let handshake = conn.comline2.as_ref().expect("the caller checked");
    let Some(message) = handshake.next_message() else {
        return COMLINE_OK;
    };
    let (head, room) = conn.split_out(out);
    if message.len() > room.len() {
        return conn.too_small(message.len(), written);
    }
    room[..message.len()].copy_from_slice(&message);
    *written = conn.finish(head, message.len());
    let handshake = conn.comline2.as_mut().expect("the caller checked");
    handshake.message_sent();
    let more = handshake.has_more();
    let refused = handshake.refusal.is_some();
    conn.sent = !more;
    if refused && !more {
        conn.closed = true;
    }
    COMLINE_OK
}

/// [`feed`]'s frame while a `comline@2` handshake isn't finished. Out of line,
/// so a handshake that runs once costs [`feed`]'s every frame nothing.
#[cold]
#[inline(never)]
pub(crate) fn feed_comline2(
    conn: &mut comline_connection,
    frame: &[u8],
    consumed: &mut usize,
    ev: &mut comline_event,
) -> comline_status {
    let handshake = conn.comline2.as_mut().expect("the caller checked");
    // A refusal only has its last message to send, and an offer goes first.
    let offering = handshake.hello_due && handshake.awaiting == Awaiting::Answer;
    if handshake.refusal.is_some() || offering {
        *consumed = 0;
        ev.frame = comline_span::default();
        return COMLINE_ERR_STATE;
    }
    let status = handshake.take(frame, ev);
    let more = handshake.has_more();
    let refused = handshake.refusal.is_some();
    let finished = handshake.awaiting == Awaiting::Nothing;
    conn.sent = !more;
    conn.received = finished && !refused;
    if refused && !more {
        conn.closed = true;
    }
    status
}
