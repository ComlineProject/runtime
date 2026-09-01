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
    };
    assert_eq!(reply, "hi, world");

    drop(client); // closes the transport → `serve` returns
    server.join().unwrap();
}
