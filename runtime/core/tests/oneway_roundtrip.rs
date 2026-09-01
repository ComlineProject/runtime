//! A one-way call (`_return: None`): the client `notify`s, the provider's
//! dispatcher runs the handler but writes no `Envelope`, and the `Server`
//! sends nothing back. Hand-written stand-in for what `comline-rust` emits
//! for a no-return `function`.
#![cfg(feature = "std")]

use std::cell::RefCell;
use std::rc::Rc;

use comline_runtime::client::Client;
use comline_runtime::contract::{BufMut, Dispatch, Kind, RuntimeError, WireFormat};
use comline_runtime::format::MsgPack;
use comline_runtime::serve::Server;
use comline_runtime::transport::duplex;
use serde::{Deserialize, Serialize};

// protocol Log { function record(line: str); }   // no `->` : one-way

#[derive(Serialize, Deserialize)]
struct RecordParams<'a> {
    #[serde(borrow)]
    line: &'a str,
}

const CALLS: &[&str] = &["record"];

trait Log {
    fn record(&self, line: &str);
}

struct LogDispatcher<T>(T);

impl<T: Log> Dispatch for LogDispatcher<T> {
    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        fmt: &W,
        _out: &mut dyn BufMut, // one-way: nothing is written here
    ) -> Result<(), RuntimeError> {
        match call.resolve(CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: RecordParams = fmt.decode(params)?;
                self.0.record(p.line);
                Ok(())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

struct Recorder(Rc<RefCell<Vec<String>>>);
impl Log for Recorder {
    fn record(&self, line: &str) {
        self.0.borrow_mut().push(line.to_string());
    }
}

#[test]
fn a_one_way_call_reaches_the_handler_and_draws_no_reply() {
    let (client_side, mut provider_side) = duplex();
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut server = Server::new(LogDispatcher(Recorder(log.clone())), MsgPack);
    let mut client = Client::new(client_side, MsgPack);

    client
        .notify(0, &RecordParams { line: "first" })
        .unwrap();
    client
        .notify(0, &RecordParams { line: "second" })
        .unwrap();

    // Two frames queued; the server pumps both, replying to neither.
    assert!(server.serve_one(&mut provider_side).unwrap());
    assert!(server.serve_one(&mut provider_side).unwrap());
    assert_eq!(&*log.borrow(), &["first".to_string(), "second".to_string()]);

    let mut buf = Vec::new();
    assert!(
        !client.transport_mut().try_recv(&mut buf).unwrap(),
        "a one-way call must not produce a response frame",
    );
}
