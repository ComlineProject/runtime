use core::cell::UnsafeCell;

struct Region(UnsafeCell<Vec<u8>>);

// SAFETY: wasm32 without the atomics feature has one thread.
unsafe impl Sync for Region {}

static IN: Region = Region(UnsafeCell::new(Vec::new()));
static OUT: Region = Region(UnsafeCell::new(Vec::new()));

/// The one place a region changes (see the module's doc): it only
/// grows, only when `len` is more than it holds, and is never shrunk
/// or freed.
fn reserve(region: &Region, len: usize) -> *mut u8 {
    // SAFETY: one thread, and no reference to the region outlives a
    // call.
    let bytes = unsafe { &mut *region.0.get() };
    if bytes.len() < len {
        let more = len - bytes.len();
        if bytes.try_reserve_exact(more).is_err() {
            return core::ptr::null_mut();
        }
        bytes.resize(len, 0);
    }
    bytes.as_mut_ptr()
}

/// A region of at least `len` bytes for input, owned by the module;
/// null if memory can't grow, and then the region is as it was.
/// Reserving more than it holds may move it (and grow memory, which
/// detaches the host's views), so a host takes the pointer again after
/// such a reserve; a reserve for no more returns the same pointer and
/// changes nothing.
#[no_mangle]
pub extern "C" fn comline_wasm_reserve_in(len: usize) -> *mut u8 {
    reserve(&IN, len)
}

/// As `comline_wasm_reserve_in`, for output.
#[no_mangle]
pub extern "C" fn comline_wasm_reserve_out(len: usize) -> *mut u8 {
    reserve(&OUT, len)
}
