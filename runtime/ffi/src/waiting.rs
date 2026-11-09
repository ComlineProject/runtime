use crate::{
    comline_connection, comline_status, with_conn, COMLINE_ERR_INVALID_ARGUMENT, COMLINE_OK,
};

/// Stop waiting on `request_id` (it timed out; the host's clock decides
/// when). A late reply to it is then `COMLINE_EVENT_STALE`. An id nobody
/// waits on is a no-op.
///
/// # Safety
/// `conn` is null or a live handle.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_abandon(
    conn: *mut comline_connection,
    request_id: u64,
) -> comline_status {
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            conn.state.abandon(request_id);
            COMLINE_OK
        })
    }
}

/// Whether any call waits on a reply: `*waiting` is 1 or 0.
///
/// # Safety
/// `conn` is null or a live handle; `waiting` is null or writable.
#[no_mangle]
pub unsafe extern "C" fn comline_connection_is_waiting(
    conn: *mut comline_connection,
    waiting: *mut u8,
) -> comline_status {
    if !disjoint!(writes [(conn, 1), (waiting, 1)]) {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    unsafe {
        with_conn(conn, |conn| {
            let Some(waiting) = waiting.as_mut() else {
                return COMLINE_ERR_INVALID_ARGUMENT;
            };
            *waiting = u8::from(conn.state.is_waiting());
            COMLINE_OK
        })
    }
}
