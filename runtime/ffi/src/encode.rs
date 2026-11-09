use crate::{
    comline_connection, comline_status, input, output, status_of, text, with_conn, Measure,
    COMLINE_ENVELOPE_ERR, COMLINE_ENVELOPE_FAULT, COMLINE_ENVELOPE_OK,
    COMLINE_ERR_INVALID_ARGUMENT, COMLINE_ERR_STATE, COMLINE_OK,
};
use comline_runtime::contract::{
    ConnectionFraming, CountBuf, Datagram2Framing, Datagram3Framing, Envelope, RuntimeError,
    SliceBuf,
};
use comline_runtime::framing::JsonRpcFraming;

/// Frame a request around a body the host's codec already wrote, with its
/// length prefix in stream mode, so `out` is ready for one transport write.
/// `*request_id` is the id it was given. A call that `expects_reply` (1)
/// waits until its reply is fed or it is abandoned; a one-way call (0) does
/// not wait.
///
/// **What each framing reads:**
/// - `datagram@2`: `protocol` and `call_id`; `name` is ignored. The frame
///   can't say one-way.
/// - `datagram@3`: the same, `protocol` and `call_id` being wire numbers.
/// - `jsonrpc@1`: `name`, escaped as a JSON string as serde_json escapes
///   one; `protocol` and `call_id` are ignored. A one-way call is a
///   notification, with no `id`. An empty body is written as `null`.
///
/// `name` is any UTF-8 text (it may be empty for `datagram@2`).
///
/// Errors, none of which spend a request id or leave a call waiting:
/// - `COMLINE_BUFFER_TOO_SMALL`: `*written` is the exact size needed;
/// - `COMLINE_ERR_FRAME_TOO_LARGE`: the frame is over `max_frame`;
/// - `COMLINE_ERR_BUSY`: `max_waiting` calls already wait;
/// - `COMLINE_ERR_INVALID_ARGUMENT`: a name that isn't UTF-8,
///   `expects_reply` not 0 or 1, or a null pointer;
/// - `COMLINE_ERR_STATE`: the handshake hasn't completed.
///
/// # Safety
/// `conn` is null or a live handle; `name`, `body` are null or readable for
/// their lengths; `out` is null or `cap` writable bytes; `written` and
/// `request_id` are null or writable.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn comline_connection_encode_request(
    conn: *mut comline_connection,
    protocol: u16,
    call_id: u16,
    name: *const u8,
    name_len: usize,
    expects_reply: u8,
    body: *const u8,
    body_len: usize,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
    request_id: *mut u64,
) -> comline_status {
    if !disjoint!(
        writes [(conn, 1), (out, cap), (written, 1), (request_id, 1)],
        reads [(name, name_len), (body, body_len)]
    ) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(body), Some(out), Some(written), Some(request_id)) = (
                input(body, body_len),
                output(out, cap),
                written.as_mut(),
                request_id.as_mut(),
            ) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *written = 0;
            let Some(name) = text(name, name_len) else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            let expects_reply = match expects_reply {
                0 => false,
                1 => true,
                _ => return COMLINE_ERR_INVALID_ARGUMENT,
            };
            if !conn.open() {
                return COMLINE_ERR_STATE;
            }
            let framing = conn.framing;
            let (head, room) = conn.split_out(out);
            let mut buf = SliceBuf::new(room);
            let encoded = with_framing!(framing, |f| conn.state.encode_request_body(
                f,
                protocol,
                call_id,
                name,
                body,
                expects_reply,
                &mut buf,
            ));
            match encoded {
                Ok(id) => {
                    let len = buf.written().len();
                    *written = conn.finish(head, len);
                    *request_id = id;
                    COMLINE_OK
                }
                Err(RuntimeError::BufferTooSmall) => {
                    // Count the frame without committing it.
                    let mut measure = Measure(CountBuf::new());
                    let again = with_framing!(framing, |f| conn.state.encode_request_body(
                        f,
                        protocol,
                        call_id,
                        name,
                        body,
                        expects_reply,
                        &mut measure,
                    ));
                    debug_assert_eq!(again, Err(RuntimeError::BufferTooSmall));
                    conn.too_small(measure.0.len(), written)
                }
                Err(error) => status_of(error),
            }
        })
    }
}

/// Frame the reply to a request this side was given, with its length prefix
/// in stream mode. `envelope` is `COMLINE_ENVELOPE_OK` (`body` is the
/// result), `COMLINE_ENVELOPE_ERR` (`error_id` is the schema-global ordinal,
/// `body` the error's fields) or `COMLINE_ENVELOPE_FAULT` (no body:
/// `body_len` must be 0).
///
/// **What each framing reads:** `datagram@2` and `datagram@3` write `protocol`
/// (the one the request was for) and the request id; `jsonrpc@1` ignores `protocol`,
/// writes an empty body as `null`, an error as a JSON-RPC `error` object with
/// `code` = `error_id`, and a fault as `-32603`.
///
/// Errors: `COMLINE_BUFFER_TOO_SMALL` (`*written` is the exact size needed),
/// `COMLINE_ERR_FRAME_TOO_LARGE`, `COMLINE_ERR_INVALID_ARGUMENT`,
/// `COMLINE_ERR_STATE` (the handshake hasn't completed).
///
/// # Safety
/// `conn` is null or a live handle; `body` is null or readable for
/// `body_len`; `out` is null or `cap` writable bytes; `written` is null or
/// writable.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn comline_connection_encode_reply(
    conn: *mut comline_connection,
    protocol: u16,
    request_id: u64,
    envelope: u8,
    error_id: u16,
    body: *const u8,
    body_len: usize,
    out: *mut u8,
    cap: usize,
    written: *mut usize,
) -> comline_status {
    if !disjoint!(
        writes [(conn, 1), (out, cap), (written, 1)],
        reads [(body, body_len)]
    ) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let (Some(body), Some(out), Some(written)) =
                (input(body, body_len), output(out, cap), written.as_mut())
            else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *written = 0;
            let envelope = match envelope {
                COMLINE_ENVELOPE_OK => Envelope::Ok(body),
                COMLINE_ENVELOPE_ERR => Envelope::Err { id: error_id, body },
                COMLINE_ENVELOPE_FAULT if body.is_empty() => Envelope::Fault,
                _ => return COMLINE_ERR_INVALID_ARGUMENT,
            };
            if !conn.open() {
                return COMLINE_ERR_STATE;
            }
            let (head, room) = conn.split_out(out);
            let mut buf = SliceBuf::new(room);
            with_framing!(conn.framing, |f| f
                .encode_reply(protocol, request_id, &envelope, &mut buf));
            if buf.overflowed() {
                let mut count = CountBuf::new();
                with_framing!(conn.framing, |f| f
                    .encode_reply(protocol, request_id, &envelope, &mut count));
                return conn.too_small(count.len(), written);
            }
            let len = buf.written().len();
            *written = conn.finish(head, len);
            COMLINE_OK
        })
    }
}
