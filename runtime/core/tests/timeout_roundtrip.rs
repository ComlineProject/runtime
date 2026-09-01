//! `Client::call_with_timeout` — what a generated stub emits for a
//! `@timeout_ms` function annotation. Over `InMemory` (which overrides
//! `Transport::recv_timeout`): the call gives up when no reply comes, and
//! succeeds normally when one does before the deadline.
#![cfg(feature = "std")]

use std::thread;
use std::time::Duration;

use comline_runtime::client::Client;
use comline_runtime::contract::{BufMut, Dispatch, Envelope, Kind, RuntimeError, WireFormat};
use comline_runtime::format::MsgPack;
use comline_runtime::serve::Server;
use comline_runtime::transport::duplex;
use serde::{Deserialize, Serialize};

// protocol Echo { function ping(n: u32) -> u32; }

#[derive(Serialize, Deserialize)]
struct PingParams {
    n: u32,
}

const CALLS: &[&str] = &["ping"];

trait Echo {
    fn ping(&self, n: u32) -> u32;
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
                let p: PingParams = fmt.decode(params)?;
                let mut body = Vec::new();
                fmt.encode(&self.0.ping(p.n), &mut body)?;
                Envelope::encode_ok(&body, out);
                Ok(())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

struct Inc;
impl Echo for Inc {
    fn ping(&self, n: u32) -> u32 {
        n + 1
    }
}

#[test]
fn a_call_that_gets_no_reply_times_out() {
    // `_provider` end stays bound (so the channel isn't disconnected) but
    // nothing serves it.
    let (client_side, _provider) = duplex();
    let mut client = Client::new(client_side, MsgPack);

    let err = client
        .call_with_timeout(0, &PingParams { n: 1 }, Duration::from_millis(50))
        .unwrap_err();
    assert_eq!(err, RuntimeError::Timeout);
}

#[test]
fn a_call_answered_before_the_deadline_succeeds() {
    let (client_side, provider_side) = duplex();

    let provider = thread::spawn(move || {
        let mut provider_side = provider_side;
        Server::new(EchoDispatcher(Inc), MsgPack)
            .serve(&mut provider_side)
            .unwrap();
    });

    let mut client = Client::new(client_side, MsgPack);

    let (reply, fmt) = client
        .call_with_timeout(0, &PingParams { n: 41 }, Duration::from_secs(5))
        .unwrap();
    let n: u32 = match reply {
        Envelope::Ok(payload) => fmt.decode(payload).unwrap(),
        Envelope::Err { .. } => panic!("unexpected error frame"),
    };
    assert_eq!(n, 42);

    drop(client);
    provider.join().unwrap();
}
