//! [`WireFormat`](crate::contract::WireFormat) implementations.
//!
//! `std`-gated for now — `rmp-serde` needs `std::io`. A `no_std` MessagePack
//! path (hand-rolled on the `no_std` `rmp` crate) comes later.

mod msgpack;

pub use msgpack::MsgPack;
