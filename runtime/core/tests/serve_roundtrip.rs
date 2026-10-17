//! A call over a real (in-process) transport: the client hand-frames a request
//! with `DatagramFraming`, the `Server` on another thread reads it, dispatches,
//! and frames the response. Exercises `Framing` + `transport::InMemory` +
//! `serve::Server` together.
#![cfg(feature = "std")]

use std::thread;

use comline_runtime::contract::{
    Call, DatagramFraming, Dispatch, Envelope, Framing, Kind, Reply, RuntimeError, WireFormat,
};
use comline_runtime::format::MsgPack;
use comline_runtime::serve::Server;
use comline_runtime::transport::{duplex, Transport};
use serde::{Deserialize, Serialize};

// protocol Greet { function hello(name: str) -> str; }

#[derive(Serialize, Deserialize)]
struct HelloParams<'a> {
    #[serde(borrow)]
    name: &'a str,
}

const CALLS: &[&str] = &["hello"];

trait Greet {
    fn hello(&self, name: &str) -> String;
}

struct GreetDispatcher<T>(T);

impl<T: Greet> Dispatch for GreetDispatcher<T> {
    fn calls(&self) -> &'static [&'static str] {
        CALLS
    }

    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        fmt: &W,
        reply: &mut Reply,
    ) -> Result<(), RuntimeError> {
        match call.resolve(CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: HelloParams = fmt.decode(params)?;
                let r = self.0.hello(p.name);
                let mut body = Vec::new();
                fmt.encode(&r, &mut body)?;
                reply.ok(&body);
                Ok(())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

struct Impl;
impl Greet for Impl {
    fn hello(&self, name: &str) -> String {
        format!("hi, {name}")
    }
}

#[test]
fn a_call_round_trips_over_the_transport() {
    let (mut client, provider) = duplex();

    let server = thread::spawn(move || {
        let mut provider = provider;
        Server::new(GreetDispatcher(Impl), MsgPack)
            .serve(&mut provider)
            .unwrap();
    });

    // client: frame `hello("world")` as request #1
    let mut request = Vec::new();
    DatagramFraming
        .encode_request(
            Call::from(0),
            1,
            &HelloParams { name: "world" },
            &MsgPack,
            &mut request,
        )
        .unwrap();
    client.send(&request).unwrap();

    // client: read the response
    let mut frame = Vec::new();
    client.recv(&mut frame).unwrap();
    let (request_id, envelope) = DatagramFraming.decode_response(&frame).unwrap();
    assert_eq!(request_id, 1);

    let reply: String = match envelope {
        Envelope::Ok(payload) => MsgPack.decode(payload).unwrap(),
        Envelope::Err { .. } => panic!("unexpected error frame"),
        Envelope::Fault => panic!("unexpected fault frame"),
    };
    assert_eq!(reply, "hi, world");

    drop(client); // closes the transport → `serve` returns
    server.join().unwrap();
}

/// A call this provider does not have used to take the whole server down: the
/// `?` on `dispatch` unwound out of `serve`, so the caller never got a frame
/// and sat there until its timeout. Now it is answered with a fault, and the
/// loop goes on to serve the next call.
#[test]
fn an_unknown_call_is_answered_with_a_fault_and_the_loop_survives() {
    let (mut client, provider) = duplex();

    let server = thread::spawn(move || {
        let mut provider = provider;
        Server::new(GreetDispatcher(Impl), MsgPack)
            .serve(&mut provider)
            .unwrap();
    });

    let frame_for = |ordinal: u16, request_id: u64| {
        let mut request = Vec::new();
        DatagramFraming
            .encode_request(
                Call::from(ordinal),
                request_id,
                &HelloParams { name: "world" },
                &MsgPack,
                &mut request,
            )
            .unwrap();
        request
    };

    // An ordinal outside the call table.
    client.send(&frame_for(9, 1)).unwrap();
    let mut frame = Vec::new();
    client.recv(&mut frame).unwrap();
    assert_eq!(
        DatagramFraming.decode_response(&frame),
        Some((1, Envelope::Fault)),
    );

    // The connection is still good.
    client.send(&frame_for(0, 2)).unwrap();
    frame.clear();
    client.recv(&mut frame).unwrap();
    let (request_id, envelope) = DatagramFraming.decode_response(&frame).unwrap();
    assert_eq!(request_id, 2);
    let reply: String = match envelope {
        Envelope::Ok(payload) => MsgPack.decode(payload).unwrap(),
        other => panic!("expected a reply, got {other:?}"),
    };
    assert_eq!(reply, "hi, world");

    drop(client);
    server.join().unwrap();
}

/// Params that will not decode are the same class of problem: one bad frame
/// should cost one call, not the connection.
#[test]
fn undecodable_params_are_answered_with_a_fault() {
    let (mut client, provider) = duplex();

    let server = thread::spawn(move || {
        let mut provider = provider;
        Server::new(GreetDispatcher(Impl), MsgPack)
            .serve(&mut provider)
            .unwrap();
    });

    // A well-formed datagram header addressing `hello`, with garbage params.
    let mut request = Vec::new();
    request.extend_from_slice(&0u16.to_le_bytes());
    request.extend_from_slice(&4u64.to_le_bytes());
    request.extend_from_slice(b"\xff\xff not msgpack");
    client.send(&request).unwrap();

    let mut frame = Vec::new();
    client.recv(&mut frame).unwrap();
    assert_eq!(
        DatagramFraming.decode_response(&frame),
        Some((4, Envelope::Fault)),
    );

    drop(client);
    server.join().unwrap();
}
