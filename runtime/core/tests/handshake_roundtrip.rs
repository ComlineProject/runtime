//! The connection handshake: `Client::connect` ⇆ `Server::serve_handshaked`
//! agree and a call goes through; a schema-hash mismatch is refused before any
//! call; `Client::new` / `Server::serve` skip it ("misaligned mode").
#![cfg(feature = "std")]

use std::thread;

use comline_runtime::client::Client;
use comline_runtime::contract::{
    BufMut, Dispatch, Envelope, Handshake, Kind, RuntimeError, WireFormat, FRAMING_DATAGRAM,
};
use comline_runtime::format::MsgPack;
use comline_runtime::serve::Server;
use comline_runtime::transport::duplex;
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
    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        fmt: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        match call.resolve(CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: BumpParams = fmt.decode(params)?;
                let mut body = Vec::new();
                fmt.encode(&self.0.bump(p.n), &mut body)?;
                Envelope::encode_ok(&body, out);
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

fn hs(ir_hash: u64) -> Handshake {
    Handshake::new(ir_hash, MsgPack.name(), FRAMING_DATAGRAM, 0)
}

const SCHEMA: u64 = 0x0011_2233_4455_6677;

#[test]
fn matching_handshakes_connect_and_a_call_round_trips() {
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve_handshaked(&mut provider_side, hs(SCHEMA))
            .unwrap();
    });

    let mut client = Client::connect(client_side, MsgPack, hs(SCHEMA)).expect("handshake");

    let (reply, fmt) = client.call(0, &BumpParams { n: 41 }).unwrap();
    let n: u32 = match reply {
        Envelope::Ok(payload) => fmt.decode(payload).unwrap(),
        Envelope::Err { .. } => panic!("unexpected error frame"),
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
            .serve_handshaked(&mut provider_side, hs(0xdead))
            .unwrap_err()
    });

    let err = match Client::connect(client_side, MsgPack, hs(SCHEMA)) {
        Err(e) => e,
        Ok(_) => panic!("connect should have refused the mismatched schema"),
    };
    assert_eq!(err, RuntimeError::Handshake);

    assert_eq!(provider.join().unwrap(), RuntimeError::Handshake);
}

#[test]
fn misaligned_mode_skips_the_handshake_entirely() {
    // `Client::new` + `Server::serve` — no handshake frame at all, the call
    // is the first thing on the wire.
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve(&mut provider_side)
            .unwrap();
    });

    let mut client = Client::new(client_side, MsgPack);
    let (reply, fmt) = client.call(0, &BumpParams { n: 7 }).unwrap();
    let n: u32 = match reply {
        Envelope::Ok(payload) => fmt.decode(payload).unwrap(),
        Envelope::Err { .. } => panic!("unexpected error frame"),
    };
    assert_eq!(n, 8);

    drop(client);
    provider.join().unwrap();
}
