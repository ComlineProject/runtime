use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use ::tokio::net::{TcpStream, ToSocketAddrs};
use bytes::Bytes;
use futures_util::{Sink, Stream};
use tokio_util::codec::{Framed, LengthDelimitedCodec, LengthDelimitedCodecError};

use crate::connection::Timer;
use crate::contract::stream::{DEFAULT_MAX_FRAME, PREFIX_CEILING, PREFIX_LEN};
use crate::contract::RuntimeError;

/// An [`AsyncTransport`](crate::transport::AsyncTransport) over a tokio TCP stream,
/// framed as the blocking [`Tcp`](crate::transport::Tcp) is (`[len: u32
/// LE][frame]`, [`contract::stream`](crate::contract::stream)), so the two
/// talk to each other.
///
/// Frames are held to the same limit, by default [`DEFAULT_MAX_FRAME`]
/// ([`with_max_frame`](Self::with_max_frame) sets another):
/// - a send over it is [`RuntimeError::FrameTooLarge`], and nothing is
///   queued or written;
/// - a received length prefix over it is
///   [`RuntimeError::FrameTooLarge`], before its body is read or
///   allocated. The stream is out of step past it, so every later
///   operation returns [`RuntimeError::Transport`]; the socket closes
///   when the `Tcp` is dropped.
pub struct Tcp {
    framed: Framed<TcpStream, LengthDelimitedCodec>,
    max_frame: usize,
    closed: bool,
}

impl Tcp {
    /// Wrap an established stream (e.g. from `TcpListener::accept`).
    pub fn new(stream: TcpStream) -> Self {
        let codec = LengthDelimitedCodec::builder()
            .length_field_length(PREFIX_LEN)
            .little_endian()
            .max_frame_length(DEFAULT_MAX_FRAME)
            .new_codec();
        Self {
            framed: Framed::new(stream, codec),
            max_frame: DEFAULT_MAX_FRAME,
            closed: false,
        }
    }

    /// Connect to `addr`.
    pub async fn connect(addr: impl ToSocketAddrs) -> Result<Self, RuntimeError> {
        TcpStream::connect(addr)
            .await
            .map(Self::new)
            .map_err(|_| RuntimeError::Transport)
    }

    /// Hold frames to `max_frame` bytes instead of [`DEFAULT_MAX_FRAME`],
    /// lower or higher. A limit above what the `u32` prefix can declare
    /// ([`PREFIX_CEILING`]) is refused with
    /// [`RuntimeError::FrameTooLarge`].
    ///
    /// An [`AsyncConnection`](crate::connection::AsyncConnection) over it checks its
    /// own sends against
    /// [`set_max_frame`](crate::connection::AsyncConnection::set_max_frame); give it
    /// the same value.
    pub fn with_max_frame(mut self, max_frame: usize) -> Result<Self, RuntimeError> {
        if max_frame > PREFIX_CEILING {
            return Err(RuntimeError::FrameTooLarge);
        }
        self.framed.codec_mut().set_max_frame_length(max_frame);
        self.max_frame = max_frame;
        Ok(self)
    }

    /// The largest frame this transport sends or receives.
    pub fn max_frame(&self) -> usize {
        self.max_frame
    }

    /// The wrapped stream.
    pub fn stream(&self) -> &TcpStream {
        self.framed.get_ref()
    }

    fn open(&self) -> Result<(), RuntimeError> {
        if self.closed {
            Err(RuntimeError::Transport)
        } else {
            Ok(())
        }
    }
}

/// The codec's own refusal of a frame over its limit, or another failure.
fn error_of(error: &io::Error) -> RuntimeError {
    match error.get_ref() {
        Some(inner) if inner.is::<LengthDelimitedCodecError>() => RuntimeError::FrameTooLarge,
        _ => RuntimeError::Transport,
    }
}

impl Stream for Tcp {
    type Item = Result<Vec<u8>, RuntimeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.closed {
            return Poll::Ready(Some(Err(RuntimeError::Transport)));
        }
        Pin::new(&mut self.framed).poll_next(cx).map(|frame| {
            frame.map(|frame| {
                frame.map(|bytes| bytes.to_vec()).map_err(|error| {
                    // Whatever went wrong, the stream can't be read on.
                    self.closed = true;
                    error_of(&error)
                })
            })
        })
    }
}

impl Sink<Vec<u8>> for Tcp {
    type Error = RuntimeError;

    fn poll_ready(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), RuntimeError>> {
        self.open()?;
        Sink::<Bytes>::poll_ready(Pin::new(&mut self.framed), cx).map_err(|e| error_of(&e))
    }

    fn start_send(mut self: Pin<&mut Self>, frame: Vec<u8>) -> Result<(), RuntimeError> {
        self.open()?;
        // Before the codec, which would refuse it too, but only once the
        // frame is in its write buffer's way.
        if frame.len() > self.max_frame {
            return Err(RuntimeError::FrameTooLarge);
        }
        Pin::new(&mut self.framed)
            .start_send(Bytes::from(frame))
            .map_err(|e| error_of(&e))
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), RuntimeError>> {
        self.open()?;
        Sink::<Bytes>::poll_flush(Pin::new(&mut self.framed), cx).map_err(|e| error_of(&e))
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), RuntimeError>> {
        Sink::<Bytes>::poll_close(Pin::new(&mut self.framed), cx).map_err(|e| error_of(&e))
    }
}

/// Tokio's clock, for [`AsyncConnection::set_timer`](crate::connection::AsyncConnection::set_timer).
pub fn timer() -> Timer {
    Arc::new(|duration| Box::pin(::tokio::time::sleep(duration)))
}
