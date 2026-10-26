//! Frames over the limit, at the default limit
//! ([`DEFAULT_MAX_FRAME`]), on both TCP transports and every driver:
//!
//! - a send over the limit is refused with nothing written, and the
//!   connection keeps working;
//! - a received length prefix over it is refused at once, before its body
//!   arrives, and the transport is closed after it;
//! - a handler's reply over it reaches the caller as a fault;
//! - the blocking and the tokio TCP transports agree on the boundary.
//!
//! These use only the default limit, so each one ran against the runtime
//! before the limit was enforced this way, and failed there.
#![cfg(feature = "std")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use comline_runtime::client::Client;
use comline_runtime::connection::Connection;
use comline_runtime::contract::stream::DEFAULT_MAX_FRAME;
use comline_runtime::contract::{
    Call, Dispatch, Envelope, Kind, Peer, Reply, RuntimeError, WireFormat,
};
use comline_runtime::format::MsgPack;
use comline_runtime::serve::Server;
use comline_runtime::transport::{Tcp, Transport};
use serde::{Deserialize, Serialize};

// protocol Echo {
//     function echo(text: str) -> str;
//     function big(len: u64) -> str;    // a reply of `len` bytes
// }

const ECHO: u16 = 0;
const ECHO_CALLS: &[&str] = &["echo", "big"];

/// How long a test waits on a socket before calling it a hang.
const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize)]
struct TextParams {
    text: String,
}

struct EchoDispatcher;

impl Dispatch for EchoDispatcher {
    fn calls(&self) -> &'static [&'static str] {
        ECHO_CALLS
    }

    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        format: &W,
        reply: &mut Reply,
    ) -> Result<(), RuntimeError> {
        let text = match call.resolve(ECHO_CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => format.decode::<TextParams>(params)?.text,
            1 => "x".repeat(format.decode::<u64>(params)? as usize),
            _ => return Err(RuntimeError::UnknownCall),
        };
        let mut body = Vec::new();
        format.encode(&text, &mut body)?;
        reply.ok(&body);
        Ok(())
    }
}

fn text(len: usize) -> TextParams {
    TextParams {
        text: "x".repeat(len),
    }
}

fn decode_text<W: WireFormat>(
    (envelope, format): (Envelope<'_>, &W),
) -> Result<String, RuntimeError> {
    match envelope {
        Envelope::Ok(payload) => format.decode(payload),
        Envelope::Fault => Err(RuntimeError::Provider),
        Envelope::Err { id, .. } => Err(RuntimeError::Remote { id }),
    }
}

/// A connected pair of sockets: (dialled, accepted).
fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dialled = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    for s in [&dialled, &accepted] {
        s.set_read_timeout(Some(PATIENCE)).unwrap();
        s.set_write_timeout(Some(PATIENCE)).unwrap();
    }
    (dialled, accepted)
}

/// One frame as the stream framing writes it.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

// ── the blocking transport ─────────────────────────────────────────────────

#[test]
fn tcp_refuses_a_send_over_the_limit_and_writes_nothing() {
    let (dialled, mut raw) = sockets();
    let mut tcp = Tcp::new(dialled);
    // Read everything the peer gets, so an oversized write can't block.
    let reader = thread::spawn(move || {
        let mut got = Vec::new();
        raw.read_to_end(&mut got).unwrap();
        got
    });

    assert_eq!(
        tcp.send(&vec![0; DEFAULT_MAX_FRAME + 1]),
        Err(RuntimeError::FrameTooLarge)
    );
    tcp.send(b"ok").unwrap();
    drop(tcp);
    assert_eq!(
        reader.join().unwrap(),
        framed(b"ok"),
        "only the frame under the limit went out"
    );
}

#[test]
fn tcp_refuses_an_oversized_prefix_at_once_then_closes() {
    let (dialled, mut raw) = sockets();
    let mut tcp = Tcp::new(dialled);

    // The prefix alone: refused without waiting for a body.
    raw.write_all(&[0xff; 4]).unwrap();
    let mut buf = Vec::new();
    let refused = tcp.recv(&mut buf);

    // What follows isn't read as a frame: the stream is out of step after
    // the refused prefix, so the transport closed.
    raw.write_all(&framed(b"x")).unwrap();
    let next = tcp.recv(&mut buf).map(|()| buf.clone());
    assert_eq!(
        (refused, next),
        (
            Err(RuntimeError::FrameTooLarge),
            Err(RuntimeError::Transport)
        )
    );
    assert_eq!(tcp.send(b"y"), Err(RuntimeError::Transport));

    // And the peer sees it close.
    let mut rest = Vec::new();
    raw.read_to_end(&mut rest)
        .expect("the peer sees the stream end, not a timeout");
    assert!(rest.is_empty());
}

// ── the blocking drivers ───────────────────────────────────────────────────

#[test]
fn a_server_reports_an_oversized_request() {
    let (dialled, mut raw) = sockets();
    let mut tcp = Tcp::new(dialled);
    raw.write_all(&[0xff; 4]).unwrap();
    let mut server = Server::new(EchoDispatcher, MsgPack);
    assert_eq!(server.serve_one(&mut tcp), Err(RuntimeError::FrameTooLarge));
}

#[test]
fn a_connection_reports_an_oversized_request() {
    let (dialled, mut raw) = sockets();
    raw.write_all(&[0xff; 4]).unwrap();
    let mut conn = Connection::new_unverified(Tcp::new(dialled), MsgPack);
    conn.provide(ECHO, EchoDispatcher);
    assert_eq!(conn.serve_one(), Err(RuntimeError::FrameTooLarge));
}

/// A blocking server thread on `accepted`, serving until the peer goes.
fn serve_connection(accepted: TcpStream) -> thread::JoinHandle<Result<(), RuntimeError>> {
    thread::spawn(move || {
        let mut conn = Connection::new_unverified(Tcp::new(accepted), MsgPack);
        conn.provide(ECHO, EchoDispatcher);
        conn.serve()
    })
}

#[test]
fn a_connection_refuses_an_oversized_call_and_keeps_working() {
    let (dialled, accepted) = sockets();
    let server = serve_connection(accepted);
    let mut conn = Connection::new_unverified(Tcp::new(dialled), MsgPack);

    let big = conn
        .call(ECHO, Call::new(0, "echo"), &text(DEFAULT_MAX_FRAME))
        .and_then(decode_text);
    let next = conn
        .call(ECHO, Call::new(0, "echo"), &text(3))
        .and_then(decode_text);
    assert_eq!(
        (big, next),
        (Err(RuntimeError::FrameTooLarge), Ok("xxx".into()))
    );

    drop(conn);
    server.join().unwrap().unwrap();
}

#[test]
fn a_connection_answers_an_oversized_reply_with_a_fault() {
    let (dialled, accepted) = sockets();
    let server = serve_connection(accepted);
    let mut conn = Connection::new_unverified(Tcp::new(dialled), MsgPack);

    let big = conn
        .call(ECHO, Call::new(1, "big"), &(DEFAULT_MAX_FRAME as u64))
        .and_then(decode_text);
    let next = conn
        .call(ECHO, Call::new(0, "echo"), &text(3))
        .and_then(decode_text);
    assert_eq!((big, next), (Err(RuntimeError::Provider), Ok("xxx".into())));

    drop(conn);
    server.join().unwrap().unwrap();
}

#[test]
fn a_server_answers_an_oversized_reply_with_a_fault_and_a_client_refuses_an_oversized_call() {
    let (dialled, accepted) = sockets();
    let server = thread::spawn(move || {
        let mut tcp = Tcp::new(accepted);
        Server::new(EchoDispatcher, MsgPack).serve_unverified(&mut tcp)
    });
    let mut client = Client::new_unverified(Tcp::new(dialled), MsgPack);

    let big_reply = client
        .call(Call::new(1, "big"), &(DEFAULT_MAX_FRAME as u64))
        .and_then(decode_text);
    let big_call = client
        .call(Call::new(0, "echo"), &text(DEFAULT_MAX_FRAME))
        .and_then(decode_text);
    let next = client
        .call(Call::new(0, "echo"), &text(3))
        .and_then(decode_text);
    assert_eq!(
        (big_reply, big_call, next),
        (
            Err(RuntimeError::Provider),
            Err(RuntimeError::FrameTooLarge),
            Ok("xxx".into())
        )
    );

    drop(client);
    server.join().unwrap().unwrap();
}

// ── the async driver, and the two transports together ──────────────────────

#[cfg(feature = "tokio")]
mod tokio_tcp {
    use super::*;

    use comline_runtime::async_connection::{
        tokio as rt_tokio, Answer, AsyncConnection, AsyncDispatch, AsyncPeer,
    };
    use futures_util::{SinkExt, StreamExt};

    struct AsyncEcho;

    impl AsyncDispatch for AsyncEcho {
        fn calls(&self) -> &'static [&'static str] {
            ECHO_CALLS
        }

        async fn dispatch<'a, P: AsyncPeer>(
            &'a self,
            call: Kind,
            params: &'a [u8],
            peer: &'a P,
        ) -> Result<Answer, RuntimeError> {
            let mut body = Vec::new();
            let mut reply = Reply::new(&mut body);
            EchoDispatcher.dispatch(call, params, peer.format(), &mut reply)?;
            Ok(Answer::ok(body))
        }
    }

    /// A tokio socket, and a blocking one at its other end to write raw bytes
    /// with.
    fn raw_pair() -> (tokio::net::TcpStream, TcpStream) {
        let (dialled, raw) = sockets();
        dialled.set_nonblocking(true).unwrap();
        (tokio::net::TcpStream::from_std(dialled).unwrap(), raw)
    }

    async fn tokio_sockets() -> (tokio::net::TcpStream, tokio::net::TcpStream) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (dialled, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(addr), listener.accept());
        (dialled.unwrap(), accepted.unwrap().0)
    }

    type Conn = AsyncConnection<rt_tokio::Tcp, MsgPack>;

    /// Two unverified async ends over tokio TCP, `Echo` on the second.
    async fn pair() -> (Conn, Conn) {
        let (dialled, accepted) = tokio_sockets().await;
        let client = AsyncConnection::new_unverified(rt_tokio::Tcp::new(dialled), MsgPack);
        let mut server = AsyncConnection::new_unverified(rt_tokio::Tcp::new(accepted), MsgPack);
        server.provide(ECHO, AsyncEcho);
        (client, server)
    }

    fn decode_owned(
        reply: Result<comline_runtime::async_connection::OwnedEnvelope, RuntimeError>,
    ) -> Result<String, RuntimeError> {
        decode_text((reply?.envelope(), &MsgPack))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_ends_with_the_error_when_the_peer_sends_an_oversized_frame() {
        let (dialled, mut raw) = raw_pair();
        let conn = AsyncConnection::new_unverified(rt_tokio::Tcp::new(dialled), MsgPack);
        raw.write_all(&[0xff; 4]).unwrap();
        let ended = tokio::time::timeout(PATIENCE, conn.run())
            .await
            .expect("refused at once");
        assert_eq!(ended, Err(RuntimeError::FrameTooLarge));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_ends_cleanly_when_the_peer_closes() {
        let (dialled, raw) = raw_pair();
        let conn = AsyncConnection::new_unverified(rt_tokio::Tcp::new(dialled), MsgPack);
        drop(raw);
        assert_eq!(conn.run().await, Ok(()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_oversized_async_call_fails_alone() {
        let (client, server) = pair().await;
        let caller = client.handle();
        let client = tokio::spawn(client.run());
        let server = tokio::spawn(server.run());

        let big = decode_owned(
            caller
                .call(ECHO, Call::new(0, "echo"), &text(DEFAULT_MAX_FRAME))
                .await,
        );
        let notified = caller.notify(ECHO, Call::new(0, "echo"), &text(DEFAULT_MAX_FRAME));
        let next = decode_owned(caller.call(ECHO, Call::new(0, "echo"), &text(3)).await);
        assert_eq!(
            (big, notified, next),
            (
                Err(RuntimeError::FrameTooLarge),
                Err(RuntimeError::FrameTooLarge),
                Ok("xxx".into())
            )
        );

        caller.close();
        client.await.unwrap().unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_oversized_async_reply_is_a_fault() {
        let (client, server) = pair().await;
        let caller = client.handle();
        let client = tokio::spawn(client.run());
        let server = tokio::spawn(server.run());

        let big = decode_owned(
            caller
                .call(ECHO, Call::new(1, "big"), &(DEFAULT_MAX_FRAME as u64))
                .await,
        );
        let next = decode_owned(caller.call(ECHO, Call::new(0, "echo"), &text(3)).await);
        assert_eq!((big, next), (Err(RuntimeError::Provider), Ok("xxx".into())));

        caller.close();
        client.await.unwrap().unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_tokio_transport_refuses_a_send_over_the_limit_and_stays_usable() {
        let (dialled, mut raw) = raw_pair();
        let mut tcp = rt_tokio::Tcp::new(dialled);
        let big = tcp.send(vec![0; DEFAULT_MAX_FRAME + 1]).await;
        let next = tcp.send(b"ok".to_vec()).await;
        assert_eq!((big, next), (Err(RuntimeError::FrameTooLarge), Ok(())));
        drop(tcp);
        let mut got = Vec::new();
        raw.read_to_end(&mut got).unwrap();
        assert_eq!(got, framed(b"ok"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_tokio_transport_refuses_an_oversized_prefix_then_closes() {
        let (dialled, mut raw) = raw_pair();
        let mut tcp = rt_tokio::Tcp::new(dialled);
        raw.write_all(&[0xff; 4]).unwrap();
        let refused = tcp.next().await;
        raw.write_all(&framed(b"x")).unwrap();
        let next = tcp.next().await;
        assert_eq!(
            (refused, next),
            (
                Some(Err(RuntimeError::FrameTooLarge)),
                Some(Err(RuntimeError::Transport))
            )
        );
        assert_eq!(tcp.send(b"y".to_vec()).await, Err(RuntimeError::Transport));
    }

    /// The blocking and the tokio transports, each way: a frame of exactly
    /// the limit crosses, and one byte more is refused by the sender.
    #[tokio::test(flavor = "multi_thread")]
    async fn blocking_and_tokio_agree_on_the_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let blocking = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(PATIENCE)).unwrap();
            let mut tcp = Tcp::new(stream);
            let mut buf = Vec::new();
            tcp.recv(&mut buf).unwrap();
            assert_eq!(buf.len(), DEFAULT_MAX_FRAME, "tokio's frame at the limit");
            tcp.recv(&mut buf).unwrap();
            assert_eq!(buf, b"after", "nothing came between");
            assert_eq!(
                tcp.send(&vec![1; DEFAULT_MAX_FRAME + 1]),
                Err(RuntimeError::FrameTooLarge)
            );
            tcp.send(&vec![2; DEFAULT_MAX_FRAME]).unwrap();
            tcp.send(b"end").unwrap();
        });

        let mut tcp = rt_tokio::Tcp::connect(addr).await.unwrap();
        tcp.send(vec![3; DEFAULT_MAX_FRAME]).await.unwrap();
        assert_eq!(
            tcp.send(vec![4; DEFAULT_MAX_FRAME + 1]).await,
            Err(RuntimeError::FrameTooLarge)
        );
        tcp.send(b"after".to_vec()).await.unwrap();
        let frame = tcp.next().await.unwrap().unwrap();
        assert_eq!(
            frame.len(),
            DEFAULT_MAX_FRAME,
            "the blocking frame at the limit"
        );
        assert!(frame.iter().all(|&b| b == 2));
        assert_eq!(tcp.next().await.unwrap().unwrap(), b"end");
        tokio::task::spawn_blocking(move || blocking.join().unwrap())
            .await
            .unwrap();
    }
}
