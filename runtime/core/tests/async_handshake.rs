//! Handshakes on an async connection: a custom [`AsyncHandshake`] runs on
//! both ends through `connect_with` / `accept_with` and a call goes through
//! after it, a refusing one refuses the connection, `none` sends nothing, and
//! `comline@1` over async sends exactly the bytes the blocking handshake
//! does: the conformance vectors.
#![cfg(feature = "async")]

use std::thread;

use comline_runtime::async_connection::{
    async_duplex, Answer, AsyncConnection, AsyncDispatch, AsyncPeer, AsyncTransport,
};
use comline_runtime::contract::{
    Call, ConnectionFraming, Datagram2Framing, Envelope, Kind, RuntimeError, WireFormat, HANDSHAKE_COMLINE,
    HANDSHAKE_NONE,
};
use comline_runtime::format::MsgPack;
use comline_runtime::handshake::{AsyncHandshake, ComlineHandshake, Facts, Handshake, NoHandshake};
use comline_runtime::transport::{duplex, Transport};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

// protocol Echo { function bump(n: u32) -> u32; }

const ECHO: u16 = 0;
const CALLS: &[&str] = &["bump"];
const IR_HASH: u64 = 0x1234;

struct Echo;

impl AsyncDispatch for Echo {
    fn calls(&self) -> &'static [&'static str] {
        CALLS
    }

    async fn dispatch<'a, P: AsyncPeer>(
        &'a self,
        call: Kind,
        params: &'a [u8],
        peer: &'a P,
    ) -> Result<Answer, RuntimeError> {
        match call.resolve(CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let n: u32 = peer.format().decode(params)?;
                let mut body = Vec::new();
                peer.format().encode(&(n + 1), &mut body)?;
                Ok(Answer::ok(body))
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

/// A third-party hello: the consumer sends `HELLO <ir_hash>`, the provider
/// answers `WELCOME` if the hash is its own and `GO AWAY` if not.
#[derive(Default)]
struct Hello {
    ran: bool,
}

impl AsyncHandshake for Hello {
    fn name(&self) -> &str {
        "com.acme.hello@1"
    }

    async fn initiate<T: AsyncTransport>(&mut self, transport: &mut T, local: &Facts<'_>) -> Result<(), RuntimeError> {
        self.ran = true;
        transport.send(format!("HELLO {}", local.ir_hash).into_bytes()).await?;
        match transport.next().await.ok_or(RuntimeError::Transport)??.as_slice() {
            b"WELCOME" => Ok(()),
            _ => Err(RuntimeError::Handshake),
        }
    }

    async fn respond<T: AsyncTransport>(&mut self, transport: &mut T, local: &Facts<'_>) -> Result<(), RuntimeError> {
        self.ran = true;
        let hello = transport.next().await.ok_or(RuntimeError::Transport)??;
        let ok = hello == format!("HELLO {}", local.ir_hash).into_bytes();
        let answer: &[u8] = if ok { b"WELCOME" } else { b"GO AWAY" };
        transport.send(answer.to_vec()).await?;
        if ok {
            Ok(())
        } else {
            Err(RuntimeError::Handshake)
        }
    }
}

/// One `bump` over a connected pair.
async fn bump_over<T: AsyncTransport>(
    client: AsyncConnection<T, MsgPack>,
    mut server: AsyncConnection<T, MsgPack>,
) -> u32 {
    server.provide(ECHO, Echo);
    let handle = client.handle();
    let client = tokio::spawn(client.run());
    let server = tokio::spawn(server.run());
    let reply = handle.call(ECHO, Call::new(0, "bump"), &41u32).await.unwrap();
    let n = match reply.envelope() {
        Envelope::Ok(payload) => MsgPack.decode(payload).unwrap(),
        _ => panic!("bump failed"),
    };
    handle.close();
    client.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
    n
}

#[tokio::test]
async fn a_custom_handshake_runs_on_both_ends_then_calls_go_through() {
    let (dialler, acceptor) = async_duplex();
    let (mut ours, mut theirs) = (Hello::default(), Hello::default());
    let (client, server) = tokio::join!(
        AsyncConnection::connect_with(dialler, MsgPack, Datagram2Framing, &mut ours, IR_HASH),
        AsyncConnection::accept_with(acceptor, MsgPack, Datagram2Framing, &mut theirs, IR_HASH),
    );
    assert!(ours.ran && theirs.ran);
    assert_eq!(AsyncHandshake::name(&ours), "com.acme.hello@1");
    assert_eq!(bump_over(client.unwrap(), server.unwrap()).await, 42);
}

#[tokio::test]
async fn a_custom_handshake_that_refuses_refuses_the_connection() {
    let (dialler, acceptor) = async_duplex();
    let (mut ours, mut theirs) = (Hello::default(), Hello::default());
    let (client, server) = tokio::join!(
        AsyncConnection::connect_with(dialler, MsgPack, Datagram2Framing, &mut ours, 1),
        AsyncConnection::accept_with(acceptor, MsgPack, Datagram2Framing, &mut theirs, 2),
    );
    assert_eq!(client.err(), Some(RuntimeError::Handshake));
    assert_eq!(server.err(), Some(RuntimeError::Handshake));
}

#[tokio::test]
async fn the_official_handshakes_run_through_the_with_constructors() {
    assert_eq!(AsyncHandshake::name(&ComlineHandshake), HANDSHAKE_COMLINE);
    assert_eq!(AsyncHandshake::name(&NoHandshake), HANDSHAKE_NONE);

    let (dialler, acceptor) = async_duplex();
    let (mut ours, mut theirs) = (ComlineHandshake, ComlineHandshake);
    let (client, server) = tokio::join!(
        AsyncConnection::connect_with(dialler, MsgPack, Datagram2Framing, &mut ours, IR_HASH),
        AsyncConnection::accept_with(acceptor, MsgPack, Datagram2Framing, &mut theirs, IR_HASH),
    );
    assert_eq!(bump_over(client.unwrap(), server.unwrap()).await, 42);

    // `none` sends nothing: the first frame the acceptor sees is the call.
    let (dialler, acceptor) = async_duplex();
    let (mut ours, mut theirs) = (NoHandshake, NoHandshake);
    let (client, server) = tokio::join!(
        AsyncConnection::connect_with(dialler, MsgPack, Datagram2Framing, &mut ours, IR_HASH),
        AsyncConnection::accept_with(acceptor, MsgPack, Datagram2Framing, &mut theirs, 999),
    );
    assert_eq!(bump_over(client.unwrap(), server.unwrap()).await, 42);
}

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    /// `handshake`, or `handshake_check`: our facts against the peer's frame.
    op: String,
    /// Absent on a `reject` case: bytes no peer may send.
    input: Option<Input>,
    bytes: String,
    #[serde(default)]
    expect: Option<String>,
}

#[derive(Deserialize)]
struct Input {
    capabilities: u32,
    framing: String,
    ir_hash: String,
    wire_format: String,
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// What `comline@1` sends over a blocking transport for `local`: the peer
/// end reads it and echoes it back so the exchange completes.
fn sync_bytes(local: Facts<'_>) -> Vec<u8> {
    let (mut ours, mut peer) = duplex();
    let echo = thread::spawn(move || {
        let mut frame = Vec::new();
        peer.recv(&mut frame).unwrap();
        peer.send(&frame).unwrap();
        frame
    });
    Handshake::initiate(&mut ComlineHandshake, &mut ours, &local).unwrap();
    echo.join().unwrap()
}

/// The same over an async transport, both sides.
async fn async_bytes(local: Facts<'_>, respond: bool) -> Vec<u8> {
    let (mut ours, mut peer) = async_duplex();
    let echo = async {
        let frame = peer.next().await.unwrap().unwrap();
        peer.send(frame.clone()).await.unwrap();
        frame
    };
    let run = async {
        if respond {
            AsyncHandshake::respond(&mut ComlineHandshake, &mut ours, &local).await
        } else {
            AsyncHandshake::initiate(&mut ComlineHandshake, &mut ours, &local).await
        }
    };
    let (ran, frame) = tokio::join!(run, echo);
    ran.unwrap();
    frame
}

#[tokio::test]
async fn comline_handshake_over_async_sends_the_vector_bytes() {
    let vectors: Vectors =
        serde_json::from_str(include_str!("../../conformance/vectors/handshake.json")).unwrap();
    assert!(!vectors.cases.is_empty());
    let (mut sent, mut rejected, mut checked) = (0, 0, 0);
    for case in &vectors.cases {
        let expected = hex(&case.bytes);
        if case.op == "handshake_check" {
            // Our facts, the peer's frame: the async exchange agrees or refuses.
            let i = case.input.as_ref().expect("a check has our facts");
            let local = Facts::new(i.ir_hash.parse().unwrap(), &i.wire_format, &i.framing)
                .with_capabilities(i.capabilities);
            let (mut ours, mut peer) = async_duplex();
            peer.send(expected).await.unwrap();
            let ran = AsyncHandshake::respond(&mut ComlineHandshake, &mut ours, &local).await;
            if case.expect.as_deref() == Some("reject") {
                assert_eq!(ran, Err(RuntimeError::Handshake), "{}", case.id);
            } else {
                assert_eq!(ran, Ok(()), "{}", case.id);
            }
            checked += 1;
            continue;
        }
        let Some(i) = &case.input else {
            // A frame no peer may send: the async exchange refuses it.
            let (mut ours, mut peer) = async_duplex();
            let local = Facts::new(IR_HASH, MsgPack.name(), Datagram2Framing.name());
            peer.send(expected).await.unwrap();
            let refused = AsyncHandshake::respond(&mut ComlineHandshake, &mut ours, &local).await;
            assert_eq!(refused, Err(RuntimeError::Handshake), "{}", case.id);
            rejected += 1;
            continue;
        };
        let local = Facts::new(i.ir_hash.parse().unwrap(), &i.wire_format, &i.framing)
            .with_capabilities(i.capabilities);
        assert_eq!(sync_bytes(local), expected, "{}: blocking", case.id);
        assert_eq!(async_bytes(local, false).await, expected, "{}: async initiate", case.id);
        assert_eq!(async_bytes(local, true).await, expected, "{}: async respond", case.id);
        sent += 1;
    }
    assert!(sent > 0 && rejected > 0 && checked > 0);
}
