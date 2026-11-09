// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline-runtime-ffi`: the C ABI over `comline-runtime`.
//!
//! It carries what is bytes and integers: the `comline@1` and `comline@2`
//! handshakes, the envelopes, the connection framings (`datagram@2`,
//! `datagram@3`, `jsonrpc@1`), the connection state (request ids, waiting calls, stale replies) and the frame
//! limits. Codecs, transports, async and dispatch stay in each host. See
//! `ComlineProject/docs` → Proposals → *Shared runtime core* → *One protocol
//! core*.
//!
//! `include/comline.h` is generated from this crate by cbindgen and checked
//! in. Its comments are these doc comments.
//!
//! # Rules every function keeps
//!
//! - **One crossing per frame.** `feed`, `encode_request` and `encode_reply`
//!   each handle one whole frame, and nothing calls back into the host.
//! - **Buffers are the caller's.** Every output goes to an `out` / `cap` /
//!   `*written` triple; every input is borrowed for the call only. Event
//!   spans are offsets into the caller's input, never pointers.
//! - **No allocation after `comline_connection_new`.** The handle is the one
//!   thing the library allocates, sized by `max_waiting`, and
//!   `comline_connection_free` frees it. One known exception, a bug:
//!   `comline_connection_feed` on a `jsonrpc@1` connection can allocate
//!   (runtime#58). And one by design: a `comline@2` connection allocates
//!   during its handshake, which holds lists whose size the peer picks; once
//!   the handshake is over, it allocates no more than any other connection.
//! - **Null pointers.** A data pointer (`in`, `body`, `name`, `out`, …) may
//!   be null only when its length or capacity is 0. Any other null pointer
//!   is `COMLINE_ERR_INVALID_ARGUMENT`, and so is a length or capacity above
//!   `PTRDIFF_MAX` (no object is that large).
//! - **Inputs and outputs must not overlap.** No buffer a call writes (`out`,
//!   `*written`, `*ev`, the handle, …) may share a byte with any buffer or
//!   out-pointer the same call takes; a call given overlapping ones returns
//!   `COMLINE_ERR_INVALID_ARGUMENT` and touches nothing. Inputs may overlap
//!   each other.
//! - **Panics never cross.** Every function runs inside `catch_unwind`. A
//!   panic is a bug: the call returns `COMLINE_ERR_POISONED`, the handle is
//!   marked poisoned and every later call on it returns the same, so the
//!   binding drops the connection. A free function returns
//!   `COMLINE_ERR_POISONED` for the call that panicked.
//!
//!   **On wasm32 the instance is poisoned, not just the handle.** There a
//!   panic doesn't unwind: it aborts, which is a trap (a
//!   `WebAssembly.RuntimeError` in JS) that `catch_unwind` can't catch, so
//!   the module never returns `COMLINE_ERR_POISONED` itself. The trap stops
//!   the module mid-call and leaves the whole instance (its stack pointer,
//!   its heap, every handle in it) unusable. The host marks the instance
//!   dead and answers every later call on it, and on every handle from it,
//!   with `COMLINE_ERR_POISONED`, never entering the module again; then it
//!   drops the instance and makes a new one. `js/comline.mjs` does this.
//! - **Threads.** A handle may move between threads, but calls on one handle
//!   must not overlap. Different handles are independent, and the free
//!   functions are pure.
//!
//! # Versioning
//!
//! The ABI has its own version, `COMLINE_ABI_MAJOR.COMLINE_ABI_MINOR`. A
//! binding checks `comline_abi_version()` at load, before any other call;
//! before 1.0, a minor counts as a major. Status codes, event kinds and flag
//! bits are append-only: a binding maps a number it doesn't know to a
//! generic error carrying it.

#![allow(non_camel_case_types)]

// cbindgen writes `include/comline.h` in source order, so these stay in the
// order of the header's sections, one per group: rustfmt sorts the `mod`s
// within a group.
mod status;

mod structs;

#[macro_use]
mod connection;

#[macro_use]
mod guards;

mod lifecycle;

mod handshake;

mod feed;

mod encode;

mod waiting;

mod comline2;

mod free;

/// Linear-memory regions for a wasm32 host, which can't pass pointers into
/// its own heap: it copies input into the `in` region and reads output from
/// the `out` region. Not in the header; wasm32 only.
///
/// What a host may rely on, so it can keep a region's address and size
/// between calls and reserve again only when a call needs more (as
/// `js/comline.mjs` does):
///
/// - A region moves, grows or is freed only inside its own reserve call,
///   and only when that call asks for more bytes than it holds. A reserve
///   for no more returns the same address and changes nothing, contents
///   included. Nothing else in the module touches a region: no ABI function
///   allocates into it, frees it or writes to it except through the
///   pointers the host passes.
/// - A region is never shrunk or freed; it lives as long as the instance.
/// - Reserving one region never moves or changes the other.
/// - Memory may grow during any call (a reserve, or an allocation such as
///   `comline_connection_new`'s), which detaches the host's views of it but
///   moves nothing: every address stays valid, so the host only takes its
///   views again.
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
mod wasm;

#[cfg(feature = "test-hooks")]
mod test_hooks;

#[cfg(test)]
mod tests;

pub use comline2::*;
pub use connection::*;
pub use encode::*;
pub use feed::*;
pub use free::*;
use guards::*;
pub use handshake::*;
pub use lifecycle::*;
pub use status::*;
pub use structs::*;
#[cfg(feature = "test-hooks")]
pub use test_hooks::*;
pub use waiting::*;

// ── versions ───────────────────────────────────────────────────────────────

/// The ABI's major version: it changes when an existing signature, struct
/// layout or code meaning changes. 0 until the ABI is frozen at 1.0.
pub const COMLINE_ABI_MAJOR: u32 = 0;
/// The ABI's minor version: it changes when functions, codes or flag bits
/// are added. Before 1.0 a binding accepts only its own minor.
pub const COMLINE_ABI_MINOR: u32 = 2;

/// The library's ABI version, `(major << 16) | minor`.
#[no_mangle]
pub extern "C" fn comline_abi_version() -> u32 {
    (COMLINE_ABI_MAJOR << 16) | COMLINE_ABI_MINOR
}
