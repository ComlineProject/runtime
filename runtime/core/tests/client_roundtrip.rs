//! The consumer side end to end: a hand-written stand-in for what
//! `comline-rust` will generate — a `Client`-backed stub — talking to a
//! `Server` over a real transport. Once over `InMemory`, once over loopback
//! `Tcp` (exercising the length-prefixed stream framing).
#![cfg(feature = "std")]

use std::net::{TcpListener, TcpStream};
use std::thread;

use comline_runtime::client::Client;
use comline_runtime::contract::{
    BufMut, CallError, Dispatch, Envelope, Kind, RuntimeError, WireFormat,
};
use comline_runtime::format::MsgPack;
use comline_runtime::serve::Server;
use comline_runtime::transport::{duplex, Tcp, Transport};
use serde::{Deserialize, Serialize};

// protocol Greet {
//     function hello(name: str) -> str ! Rude;
// }

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
const ERR_RUDE: u16 = 0; // schema-global error ordinal

/// The generated per-function error enum for `hello`.
#[derive(Debug, PartialEq, Eq)]
enum GreetHelloError {
    Rude(Rude),
}

// ── provider ──────────────────────────────────────────────────────────────

trait Greet {
    fn hello(&self, name: &str) -> Result<String, GreetHelloError>;
}

struct GreetDispatcher<T>(T);

impl<T: Greet> Dispatch for GreetDispatcher<T> {
    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        fmt: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        match call.resolve(CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: HelloParams = fmt.decode(params)?;
                match self.0.hello(p.name) {
                    Ok(reply) => {
                        let mut body = Vec::new();
                        fmt.encode(&reply, &mut body)?;
                        Envelope::encode_ok(&body, out);
                    }
                    Err(GreetHelloError::Rude(e)) => {
                        let mut body = Vec::new();
                        fmt.encode(&e, &mut body)?;
                        Envelope::encode_err(ERR_RUDE, &body, out);
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
                reason: "no name given".into(),
            }));
        }
        Ok(format!("hi, {name}"))
    }
}

// ── consumer: stand-in for the generated `GreetClient` ────────────────────

struct GreetClient<T, W>(Client<T, W>);

impl<T: Transport, W: WireFormat> GreetClient<T, W> {
    fn hello(&mut self, name: &str) -> Result<String, CallError<GreetHelloError>> {
        let (reply, fmt) = self.0.call(0, &HelloParams { name })?;
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

// ── the tests ────────────────────────────────────────────────────────────

#[test]
fn a_call_and_a_raised_error_round_trip_over_in_memory() {
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(GreetDispatcher(Polite), MsgPack)
            .serve(&mut provider_side)
            .unwrap();
    });

    let mut client = GreetClient(Client::new(client_side, MsgPack));

    assert_eq!(client.hello("world").unwrap(), "hi, world");
    assert_eq!(
        client.hello("").unwrap_err(),
        CallError::App(GreetHelloError::Rude(Rude {
            reason: "no name given".into(),
        })),
    );

    drop(client); // closes the transport -> `serve` returns
    provider.join().unwrap();
}

#[test]
fn a_call_round_trips_over_loopback_tcp() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let provider = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut transport = Tcp::new(stream);
        Server::new(GreetDispatcher(Polite), MsgPack)
            .serve(&mut transport)
            .unwrap();
    });

    let mut client = GreetClient(Client::new(Tcp::connect(addr).unwrap(), MsgPack));

    assert_eq!(client.hello("over tcp").unwrap(), "hi, over tcp");
    assert_eq!(client.hello("again").unwrap(), "hi, again"); // second call, same framing

    drop(client); // half-closes -> provider's `read_exact` hits EOF -> `serve` returns
    provider.join().unwrap();
}

/// A stream `Transport` must not confuse two frames that arrive back to back.
#[test]
fn tcp_framing_keeps_frame_boundaries() {
    let (a, b) = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (Tcp::new(client), Tcp::new(server))
    };
    let mut a = a;
    let mut b = b;

    a.send(b"first").unwrap();
    a.send(b"second and longer").unwrap();

    let mut buf = Vec::new();
    b.recv(&mut buf).unwrap();
    assert_eq!(buf, b"first");
    b.recv(&mut buf).unwrap();
    assert_eq!(buf, b"second and longer");
}
