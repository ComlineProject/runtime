use crate::comline_connection;

/// Make the next call on `conn` panic, inside its guard: the test hook for
/// the poisoning rule. Only with the `test-hooks` feature; not in the
/// header.
///
/// cbindgen:ignore
///
/// # Safety
/// `conn` is null or a live handle.
#[cfg(feature = "test-hooks")]
#[no_mangle]
pub unsafe extern "C" fn comline_test_panic_next(conn: *mut comline_connection) {
    // SAFETY: the caller's contract.
    if let Some(conn) = unsafe { conn.as_mut() } {
        conn.panic_next = true;
    }
}
