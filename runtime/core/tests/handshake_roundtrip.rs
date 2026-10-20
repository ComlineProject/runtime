//! The connection handshake: `Client::connect` ⇆ `Server::serve` (`comline@1`)
//! agree and a call goes through; a schema-hash mismatch is refused before any
//! call; `Client::new_unverified` / `Server::serve_unverified` skip it, as does
//! the declared `none`; and a custom handshake plugs in through
//! `connect_with` / `serve_with`.
#![cfg(feature = "std")]

use std::thread;

use comline_runtime::client::Client;
use comline_runtime::contract::{
    DatagramFraming, Dispatch, Envelope, Kind, Reply, RuntimeError, WireFormat, HANDSHAKE_COMLINE,
    HANDSHAKE_NONE,
};
use comline_runtime::format::MsgPack;
use comline_runtime::handshake::{ComlineHandshake, Facts, Handshake, NoHandshake};
use comline_runtime::serve::Server;
use comline_runtime::transport::{duplex, Transport};
use serde::{Deserialize, Serialize};

// protocol Echo { function bump(n: u32) -> u32; }

#[derive(Serialize, Deserialize)]
struct BumpParams {
    n: u32,
}

const CALLS: &[&str] = &["bump"];

trait Echo {
    fn bump(&self, n: u32) -> u32;
}

struct EchoDispatcher<T>(T);

impl<T: Echo> Dispatch for EchoDispatcher<T> {
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
                let p: BumpParams = fmt.decode(params)?;
                let mut body = Vec::new();
                fmt.encode(&self.0.bump(p.n), &mut body)?;
                reply.ok(&body);
                Ok(())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

struct Inc;
impl Echo for Inc {
    fn bump(&self, n: u32) -> u32 {
        n + 1
    }
}

const SCHEMA: u64 = 0x0011_2233_4455_6677;

#[test]
fn matching_handshakes_connect_and_a_call_round_trips() {
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve(&mut provider_side, SCHEMA)
            .unwrap();
    });

    let mut client = Client::connect(client_side, MsgPack, SCHEMA).expect("handshake");

    let (reply, fmt) = client.call(0, &BumpParams { n: 41 }).unwrap();
    let n: u32 = match reply {
        Envelope::Ok(payload) => fmt.decode(payload).unwrap(),
        Envelope::Err { .. } => panic!("unexpected error frame"),
        Envelope::Fault => panic!("unexpected fault frame"),
    };
    assert_eq!(n, 42);

    drop(client);
    provider.join().unwrap();
}

#[test]
fn a_schema_hash_mismatch_is_refused_before_any_call() {
    let (client_side, provider_side) = duplex();

    // The provider speaks a different schema version.
    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve(&mut provider_side, 0xdead)
            .unwrap_err()
    });

    let err = match Client::connect(client_side, MsgPack, SCHEMA) {
        Err(e) => e,
        Ok(_) => panic!("connect should have refused the mismatched schema"),
    };
    assert_eq!(err, RuntimeError::Handshake);

    assert_eq!(provider.join().unwrap(), RuntimeError::Handshake);
}

#[test]
fn the_unverified_paths_skip_the_handshake_entirely() {
    // `Client::new_unverified` + `Server::serve_unverified` — no handshake
    // frame at all, the call
    // is the first thing on the wire.
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve_unverified(&mut provider_side)
            .unwrap();
    });

    let mut client = Client::new_unverified(client_side, MsgPack);
    let (reply, fmt) = client.call(0, &BumpParams { n: 7 }).unwrap();
    let n: u32 = match reply {
        Envelope::Ok(payload) => fmt.decode(payload).unwrap(),
        Envelope::Err { .. } => panic!("unexpected error frame"),
        Envelope::Fault => panic!("unexpected fault frame"),
    };
    assert_eq!(n, 8);

    drop(client);
    provider.join().unwrap();
}

fn bump_once<T: Transport>(client: &mut Client<T, MsgPack>, n: u32) -> u32 {
    match client.call(0, &BumpParams { n }).unwrap() {
        (Envelope::Ok(payload), fmt) => fmt.decode(payload).unwrap(),
        _ => panic!("unexpected reply"),
    }
}

#[test]
fn the_declared_none_sends_nothing_and_checks_nothing() {
    let (client_side, provider_side) = duplex();

    // Different schema hashes: `none` doesn't look.
    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve_with(&mut provider_side, &mut NoHandshake, 0xdead)
            .unwrap();
    });

    let mut client = Client::connect_with(
        client_side,
        MsgPack,
        DatagramFraming,
        &mut NoHandshake,
        SCHEMA,
    )
    .expect("none never refuses");
    assert_eq!(bump_once(&mut client, 1), 2);

    drop(client);
    provider.join().unwrap();
}

/// A legacy peer's own hello: the consumer sends `HELLO <framing>`, the
/// provider answers `OK` if it speaks that framing. Checks no schema at all.
struct Hello;

impl Handshake for Hello {
    fn name(&self) -> &str {
        "com.example.hello@1"
    }

    fn initiate<T: Transport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        transport.send(format!("HELLO {}", local.framing).as_bytes())?;
        let mut answer = Vec::new();
        transport.recv(&mut answer)?;
        if answer == b"OK" {
            Ok(())
        } else {
            Err(RuntimeError::Handshake)
        }
    }

    fn respond<T: Transport>(
        &mut self,
        transport: &mut T,
        local: &Facts<'_>,
    ) -> Result<(), RuntimeError> {
        let mut hello = Vec::new();
        transport.recv(&mut hello)?;
        if hello != format!("HELLO {}", local.framing).as_bytes() {
            transport.send(b"NO")?;
            return Err(RuntimeError::Handshake);
        }
        transport.send(b"OK")
    }
}

#[test]
fn a_custom_handshake_runs_in_place_of_comlines() {
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve_with(&mut provider_side, &mut Hello, SCHEMA)
            .unwrap();
    });

    let mut client =
        Client::connect_with(client_side, MsgPack, DatagramFraming, &mut Hello, SCHEMA)
            .expect("hello");
    assert_eq!(bump_once(&mut client, 9), 10);

    drop(client);
    provider.join().unwrap();
}

#[test]
fn handshakes_carry_their_versioned_names() {
    assert_eq!(ComlineHandshake.name(), HANDSHAKE_COMLINE);
    assert_eq!(HANDSHAKE_COMLINE, "comline@1");
    assert_eq!(NoHandshake.name(), HANDSHAKE_NONE);
    assert_eq!(Hello.name(), "com.example.hello@1");
}
