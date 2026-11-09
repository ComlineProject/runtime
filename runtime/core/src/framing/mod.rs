//! Alternative [`Framing`](crate::contract::Framing) implementations.
//!
//! The Comline datagram framing is `DatagramFraming` in `contract`; this module
//! holds ones that need `alloc` and extra deps (the `jsonrpc` feature, which
//! `std` implies).

mod jsonrpc;

pub use jsonrpc::JsonRpcFraming;
