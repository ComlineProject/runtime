// `no_std`-first: the crate is `#![no_std]` unless the `std` feature is on
// (it is, by default). `--no-default-features` builds just `contract`.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

// The `core ↔ target` contract — `no_std`, allocation-free.
pub mod contract;

// `WireFormat` implementations (`std`-gated for now — see the module).
#[cfg(feature = "std")]
pub mod format;

// The `std` layer: transport, async, the setup builders, the dylib ABI.
#[cfg(feature = "std")]
pub mod package_abi;
#[cfg(feature = "std")]
pub mod setup;
