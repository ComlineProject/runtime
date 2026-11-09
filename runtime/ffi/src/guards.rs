use core::slice;
use std::panic::{self, AssertUnwindSafe};

use crate::{
    comline_connection, comline_span, comline_status, COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_POISONED, COMLINE_ERR_STATE, COMLINE_EVENT_BODY_ABSENT, COMLINE_EVENT_NAME_ESCAPED,
};
use comline_runtime::contract::MethodName;

/// Runs a free function's body, turning a panic into
/// `COMLINE_ERR_POISONED`.
pub(crate) fn guarded(body: impl FnOnce() -> comline_status) -> comline_status {
    panic::catch_unwind(AssertUnwindSafe(body)).unwrap_or(COMLINE_ERR_POISONED)
}

/// Runs a handle's function: refuses a null or poisoned handle, and poisons
/// it if `body` panics.
///
/// # Safety
/// `conn` is null or a live handle from `comline_connection_new`, with no
/// other call on it running.
pub(crate) unsafe fn with_conn(
    conn: *mut comline_connection,
    body: impl FnOnce(&mut comline_connection) -> comline_status,
) -> comline_status {
    if conn.is_null() {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    if unsafe { (*conn).poisoned } {
        return COMLINE_ERR_POISONED;
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller's contract; this is the only reference.
        let conn = unsafe { &mut *conn };
        #[cfg(feature = "test-hooks")]
        if core::mem::take(&mut conn.panic_next) {
            panic!("comline_test_panic_next");
        }
        if conn.closed {
            return COMLINE_ERR_STATE;
        }
        body(conn)
    }));
    match result {
        Ok(status) => status,
        Err(_) => {
            // SAFETY: the reference made inside the closure is gone.
            unsafe { (*conn).poisoned = true };
            COMLINE_ERR_POISONED
        }
    }
}

/// [`with_conn`], for what also holds on a closed handle: reading why it closed.
///
/// # Safety
/// As [`with_conn`].
pub(crate) unsafe fn with_closed_too(
    conn: *mut comline_connection,
    body: impl FnOnce(&mut comline_connection) -> comline_status,
) -> comline_status {
    if conn.is_null() {
        return COMLINE_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller's contract.
    if unsafe { (*conn).poisoned } {
        return COMLINE_ERR_POISONED;
    }
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller's contract; this is the only reference.
        body(unsafe { &mut *conn })
    }));
    result.unwrap_or_else(|_| {
        // SAFETY: the reference made inside the closure is gone.
        unsafe { (*conn).poisoned = true };
        COMLINE_ERR_POISONED
    })
}

/// `len` bytes at `ptr`, which may be null (or dangling) when `len` is 0.
///
/// # Safety
/// Unless null, `ptr` points to `len` readable bytes that stay unchanged
/// for `'a`.
pub(crate) unsafe fn input<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len > isize::MAX as usize {
        None
    } else if len == 0 {
        Some(&[])
    } else if ptr.is_null() {
        None
    } else {
        // SAFETY: the caller's contract.
        Some(unsafe { slice::from_raw_parts(ptr, len) })
    }
}

/// `cap` writable bytes at `ptr`, which may be null when `cap` is 0.
///
/// # Safety
/// Unless null, `ptr` points to `cap` writable bytes nothing else uses for
/// `'a`.
pub(crate) unsafe fn output<'a>(ptr: *mut u8, cap: usize) -> Option<&'a mut [u8]> {
    if cap > isize::MAX as usize {
        None
    } else if cap == 0 {
        Some(&mut [])
    } else if ptr.is_null() {
        None
    } else {
        // SAFETY: the caller's contract.
        Some(unsafe { slice::from_raw_parts_mut(ptr, cap) })
    }
}

/// Memory one call reads or writes: the addresses `start..end`. Empty when
/// `end == start`.
#[derive(Clone, Copy)]
pub(crate) struct Region {
    start: usize,
    end: usize,
}

/// `len` values of `T` at `ptr`. A null pointer is no memory: the call
/// refuses it on its own. A length past the address space ends at its top
/// (the call refuses any length over `isize::MAX` on its own too).
#[inline(always)]
pub(crate) fn region<T>(ptr: *const T, len: usize) -> Region {
    let start = ptr as usize;
    let len = len.saturating_mul(core::mem::size_of::<T>());
    Region {
        start,
        end: if start == 0 {
            0
        } else {
            start.saturating_add(len)
        },
    }
}

/// Whether `a` and `b` share a byte: the later start is before the earlier
/// end, which an empty region's never is.
#[inline(always)]
pub(crate) fn overlap(a: Region, b: Region) -> bool {
    a.start.max(b.start) < a.end.min(b.end)
}

/// Whether no memory a call writes shares a byte with any other memory it
/// takes. A `&mut` that aliases another reference is undefined behaviour,
/// so this is checked on addresses alone, before any reference is made.
///
/// `disjoint!(writes [(ptr, len), ...], reads [(ptr, len), ...])`, lengths
/// counted in the pointer's type. Every call runs it, so it expands to the
/// pairs themselves, without a loop: each written region against every
/// region after it, and no pair of two reads.
macro_rules! disjoint {
    (writes [$($w:expr),+ $(,)?], reads [$($r:expr),* $(,)?]) => {
        !disjoint!(@shared [$($crate::guards::region($w.0, $w.1)),+] [$($crate::guards::region($r.0, $r.1)),*])
    };
    (writes [$($w:expr),+ $(,)?]) => {
        disjoint!(writes [$($w),+], reads [])
    };
    (@shared [$w:expr] [$($r:expr),*]) => {
        false $(| $crate::guards::overlap($w, $r))*
    };
    (@shared [$w:expr, $($ws:expr),+] [$($r:expr),*]) => {
        false $(| $crate::guards::overlap($w, $ws))* $(| $crate::guards::overlap($w, $r))*
            | disjoint!(@shared [$($ws),+] [$($r),*])
    };
}

/// UTF-8 text of `len` bytes at `ptr`.
///
/// # Safety
/// As [`input`].
pub(crate) unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    // SAFETY: the caller's contract.
    core::str::from_utf8(unsafe { input(ptr, len) }?).ok()
}

/// Where `part` lies in `base`, or `None` when it isn't inside it (a
/// JSON-RPC body that was absent, which the framing gives as a static
/// `null`).
pub(crate) fn span_in(base: &[u8], part: &[u8]) -> Option<comline_span> {
    let start = (part.as_ptr() as usize).checked_sub(base.as_ptr() as usize)?;
    let end = start.checked_add(part.len())?;
    if end > base.len() {
        return None;
    }
    // `start` and the length are each at most `end`, so one check covers
    // both casts. It never fails: a frame starts at 0 or after its prefix
    // and is at most `max_frame`, which is at most the ceiling, so anything
    // in it ends by `PREFIX_LEN + COMLINE_MAX_FRAME_CEILING == u32::MAX`.
    u32::try_from(end).expect("spans fit u32 under the frame ceiling");
    Some(comline_span {
        offset: start as u32,
        len: part.len() as u32,
    })
}

/// A body's span and, when the frame had none, `COMLINE_EVENT_BODY_ABSENT`.
pub(crate) fn body_span(base: &[u8], body: &[u8]) -> (comline_span, u8) {
    match span_in(base, body) {
        Some(span) => (span, 0),
        None => (comline_span::default(), COMLINE_EVENT_BODY_ABSENT),
    }
}

/// A method name's span as the frame spells it, and
/// `COMLINE_EVENT_NAME_ESCAPED` when that spelling has escapes.
pub(crate) fn name_span(base: &[u8], name: MethodName<'_>) -> (comline_span, u8) {
    let (written, escaped) = name.as_written();
    let span = span_in(base, written.as_bytes()).expect("a name borrows the frame");
    (
        span,
        if escaped {
            COMLINE_EVENT_NAME_ESCAPED
        } else {
            0
        },
    )
}
