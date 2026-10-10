//! Opening the byte stream to the hub, before TLS.
//!
//! Behind a trait so that tests connect through an in-memory pipe they can count, cut and refuse, and the production
//! build connects with TCP. TLS sits on top of whatever this returns, in both cases, so a test that counts the bytes
//! counts exactly what a network would carry.

use std::fmt;
use std::io;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

/// A byte stream the transport can speak TLS over.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Io for T {}

pub type BoxedIo = Box<dyn Io>;

/// How long a TCP connect may take (rule 5). The whole connection attempt, TLS included, has its own limit above this.
pub const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[async_trait]
pub trait Dialer: Send + Sync + fmt::Debug {
    /// A connected stream to `host:port`. `host` is a name or an address without brackets.
    async fn dial(&self, host: &str, port: u16) -> io::Result<BoxedIo>;
}

/// TCP, for production.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpDialer;

#[async_trait]
impl Dialer for TcpDialer {
    async fn dial(&self, host: &str, port: u16) -> io::Result<BoxedIo> {
        let stream = tokio::time::timeout(TCP_CONNECT_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        // The stream carries small, latency-sensitive frames (acknowledgements, results), not bulk data.
        stream.set_nodelay(true)?;
        Ok(Box::new(stream))
    }
}
