//! The framing axis is pluggable: the exact same `Client` / `Server` / dispatch
//! code, over `JsonRpcFraming` + `Json` instead of the datagram default. A call,
//! a raised typed error, and a peek at the actual bytes (they're JSON-RPC 2.0).
#![cfg(feature = "std")]

use std::thread;

use comline_runtime::client::Client;
use comline_runtime::contract::{
    Call, CallError, Dispatch, Envelope, Kind, Reply, RuntimeError, WireFormat,
};
use comline_runtime::format::Json;
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime::serve::Server;
use comline_runtime::transport::{duplex, Transport};
use serde::{Deserialize, Serialize};

// protocol Greet { function hello(name: str) -> str ! Rude; }

#[derive(Serialize, Deserialize)]
struct HelloParams<'a> {
    #[serde(borrow)]
    name: &'a str,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct Rude {
    reason: String,
}

const CALLS: &[&str] = &["hello"];
const ERR_RUDE: u16 = 0;

#[derive(Debug, PartialEq, Eq)]
enum GreetHelloError {
    Rude(Rude),
}

trait Greet {
    fn hello(&self, name: &str) -> Result<String, GreetHelloError>;
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
                match self.0.hello(p.name) {
                    Ok(r) => {
                        let mut body = Vec::new();
                        fmt.encode(&r, &mut body)?;
                        reply.ok(&body);
                    }
                    Err(GreetHelloError::Rude(e)) => {
                        let mut body = Vec::new();
                        fmt.encode(&e, &mut body)?;
                        reply.err(ERR_RUDE, &body);
                    }
                }
                Ok(())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

struct Polite;
impl Greet for Polite {
    fn hello(&self, name: &str) -> Result<String, GreetHelloError> {
        if name.is_empty() {
            return Err(GreetHelloError::Rude(Rude {
                reason: "no name".into(),
            }));
        }
        Ok(format!("hi, {name}"))
    }
}

/// Stand-in for the generated `GreetClient` — note it passes `Call::new(id,
/// name)`; the name is what a name-oriented framing needs.
struct GreetClient<T, W, F>(Client<T, W, F>);

impl<T, W, F> GreetClient<T, W, F>
where
    T: Transport,
    W: WireFormat,
    F: comline_runtime::contract::Framing,
{
    fn hello(&mut self, name: &str) -> Result<String, CallError<GreetHelloError>> {
        let (reply, fmt) = self.0.call(Call::new(0, "hello"), &HelloParams { name })?;
        match reply {
            Envelope::Ok(payload) => fmt.decode(payload).map_err(CallError::Runtime),
            Envelope::Err { id: ERR_RUDE, body } => {
                let e: Rude = fmt.decode(body)?;
                Err(CallError::App(GreetHelloError::Rude(e)))
            }
            Envelope::Err { id, .. } => Err(CallError::Runtime(RuntimeError::Remote { id })),
        }
    }
}

#[test]
fn a_call_and_an_error_round_trip_over_json_rpc() {
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::with_framing(GreetDispatcher(Polite), Json, JsonRpcFraming)
            .serve(&mut provider_side)
            .unwrap();
    });

    let mut client = GreetClient(Client::with_framing(client_side, Json, JsonRpcFraming));

    assert_eq!(client.hello("world").unwrap(), "hi, world");
    assert_eq!(
        client.hello("").unwrap_err(),
        CallError::App(GreetHelloError::Rude(Rude {
            reason: "no name".into(),
        })),
    );

    drop(client);
    provider.join().unwrap();
}

#[test]
fn the_bytes_on_the_wire_are_json_rpc() {
    let (a, mut b) = duplex();
    let mut client = Client::with_framing(a, Json, JsonRpcFraming);

    // fire a request; don't wait for a reply (nobody serving `b` yet)
    client
        .notify(Call::new(0, "hello"), &HelloParams { name: "x" })
        .unwrap();

    let mut frame = Vec::new();
    b.recv(&mut frame).unwrap();
    assert_eq!(
        std::str::from_utf8(&frame).unwrap(),
        r#"{"jsonrpc":"2.0","method":"hello","params":{"name":"x"},"id":0}"#
    );
}
