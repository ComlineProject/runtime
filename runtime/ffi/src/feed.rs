use crate::{
    body_span, comline_connection, comline_event, comline_status, feed_comline2, input, name_span,
    span_in, status_of, with_conn, COMLINE_ENVELOPE_ERR, COMLINE_ENVELOPE_FAULT,
    COMLINE_ENVELOPE_OK, COMLINE_ERR_FRAME_TOO_LARGE, COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_EVENT_CALL_BY_NAME, COMLINE_EVENT_HANDSHAKE_OK, COMLINE_EVENT_HAS_PROTOCOL,
    COMLINE_EVENT_ONE_WAY, COMLINE_EVENT_REPLY, COMLINE_EVENT_REQUEST, COMLINE_EVENT_STALE,
    COMLINE_NEED_MORE, COMLINE_OK,
};
use comline_runtime::connection::Event;
use comline_runtime::contract::stream::{self, Split};
use comline_runtime::contract::{Datagram2Framing, Datagram3Framing, Envelope, RequestCall};
use comline_runtime::framing::JsonRpcFraming;

/// Feed received bytes and get the event for the next frame. One frame per
/// call.
///
/// - **Stream mode:** `in` is whatever the host has read so far. If a whole
///   length-prefixed frame is at its start, it is decoded, `*consumed` is
///   the prefix plus the frame, and `*ev` describes it, with spans relative
///   to `in`. Bytes after it are left for the next call. Otherwise
///   `COMLINE_NEED_MORE`, with `*consumed` 0 and `ev->frame.len` the total
///   needed once the prefix is complete (0 before).
/// - **Message mode:** `in` is exactly one frame, and `*consumed` is its
///   length.
///
/// On a `comline@1` connection the first frame is the peer's handshake:
/// `COMLINE_EVENT_HANDSHAKE_OK`, or `COMLINE_ERR_HANDSHAKE` and the
/// connection closes.
///
/// On a `comline@2` connection the first frames are the handshake's:
/// `COMLINE_EVENT_HANDSHAKE_CONTINUE` while it goes on,
/// `COMLINE_EVENT_HANDSHAKE_OK` when it's done, or `COMLINE_ERR_REFUSED`
/// (`comline_connection_refusal`). `COMLINE_EVENT_SEND` on any of them means
/// `comline_connection_start` has a message to send. Fed while its own offer
/// hasn't been written, or after a refusal, it's `COMLINE_ERR_STATE` with
/// nothing consumed.
///
/// After that, a frame is a request, a reply to a waiting call (which then
/// waits no more), or a stale reply. A malformed frame is
/// `COMLINE_ERR_FRAMING` with the frame consumed. In stream mode, a prefix
/// over `max_frame` is `COMLINE_ERR_FRAME_TOO_LARGE` and closes the
/// connection.
///
/// **What each framing reads:**
/// - `datagram@2`: the protocol number (`COMLINE_EVENT_HAS_PROTOCOL`), the
///   `call_id` and the request id. It can't mark a call one-way.
/// - `datagram@3`: the same, the protocol and `call_id` being wire numbers,
///   each written as a shortest-form varint.
/// - `jsonrpc@1`: the method (`COMLINE_EVENT_CALL_BY_NAME`; with
///   `COMLINE_EVENT_NAME_ESCAPED` when the frame spelled it with escapes) and
///   the `id`. A notification is `COMLINE_EVENT_ONE_WAY` with request id 0.
///   Absent `params`, `result` or `error.data` is
///   `COMLINE_EVENT_BODY_ABSENT`.
///
/// **Known bug (runtime#58):** on `jsonrpc@1` this call can allocate, which
/// breaks the rule that nothing allocates after `comline_connection_new`.
/// The JSON parser allocates for any refused frame, for a member name
/// written with escapes, and for brackets nested three deep in the frame
/// (`"params":[[1]]`, counting the frame's own `{`). `datagram@2` never
/// allocates here.
///
/// # Safety
/// `conn` is null or a live handle; `in` is null or `in_len` readable bytes;
/// `consumed` and `ev` are null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_feed(
    conn: *mut comline_connection,
    in_: *const u8,
    in_len: usize,
    consumed: *mut usize,
    ev: *mut comline_event,
) -> comline_status {
    if !disjoint!(
        writes [(conn, 1), (consumed, 1), (ev, 1)],
        reads [(in_, in_len)]
    ) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(input), Some(consumed), Some(ev)) =
                (input(in_, in_len), consumed.as_mut(), ev.as_mut())
            else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *consumed = 0;
            *ev = comline_event::default();
            feed(conn, input, consumed, ev)
        })
    }
}

pub(crate) fn feed(
    conn: &mut comline_connection,
    input: &[u8],
    consumed: &mut usize,
    ev: &mut comline_event,
) -> comline_status {
    let (start, len) = if conn.stream {
        match stream::split_frame(input, conn.max_frame) {
            Split::Frame { start, len } => (start, len),
            Split::NeedMore { needed } => {
                ev.frame.len = u32::try_from(needed).expect("a prefix is at most the ceiling");
                return COMLINE_NEED_MORE;
            }
            Split::TooLarge => {
                conn.closed = true;
                return COMLINE_ERR_FRAME_TOO_LARGE;
            }
        }
    } else {
        if input.len() > conn.max_frame {
            return COMLINE_ERR_FRAME_TOO_LARGE;
        }
        (0, input.len())
    };
    let frame = &input[start..start + len];
    *consumed = start + len;
    ev.frame = span_in(input, frame).expect("the frame is in the input");

    if !conn.received {
        if conn.comline2.is_some() {
            return feed_comline2(conn, frame, consumed, ev);
        }
        let ours = conn.ours.expect("only a handshake leaves `received` unset");
        return match ours.check_bytes(frame) {
            Ok(peer) => {
                conn.received = true;
                ev.kind = COMLINE_EVENT_HANDSHAKE_OK;
                ev.capabilities = peer.capabilities;
                COMLINE_OK
            }
            Err(error) => {
                conn.closed = true;
                status_of(error)
            }
        };
    }

    let event = match with_framing!(conn.framing, |f| conn.state.receive(f, frame)) {
        Ok(event) => event,
        Err(error) => return status_of(error),
    };
    match event {
        Event::Request {
            protocol,
            request,
            one_way,
        } => {
            ev.kind = COMLINE_EVENT_REQUEST;
            if let Some(protocol) = protocol {
                ev.flags |= COMLINE_EVENT_HAS_PROTOCOL;
                ev.protocol = protocol;
            }
            if one_way {
                ev.flags |= COMLINE_EVENT_ONE_WAY;
            }
            match request.call {
                RequestCall::Id(id) => ev.call_id = id,
                RequestCall::Name(name) => {
                    let (span, escaped) = name_span(input, name);
                    ev.flags |= COMLINE_EVENT_CALL_BY_NAME | escaped;
                    ev.call_name = span;
                }
            }
            ev.request_id = request.request_id;
            let (body, absent) = body_span(input, request.params);
            ev.body = body;
            ev.flags |= absent;
        }
        Event::Reply {
            request_id,
            envelope,
        } => {
            ev.kind = COMLINE_EVENT_REPLY;
            ev.request_id = request_id;
            let body = match envelope {
                Envelope::Ok(payload) => {
                    ev.envelope = COMLINE_ENVELOPE_OK;
                    Some(payload)
                }
                Envelope::Err { id, body } => {
                    ev.envelope = COMLINE_ENVELOPE_ERR;
                    ev.error_id = id;
                    Some(body)
                }
                Envelope::Fault => {
                    ev.envelope = COMLINE_ENVELOPE_FAULT;
                    None
                }
            };
            if let Some(body) = body {
                let (body, absent) = body_span(input, body);
                ev.body = body;
                ev.flags |= absent;
            }
        }
        Event::Stale => ev.kind = COMLINE_EVENT_STALE,
    }
    COMLINE_OK
}
