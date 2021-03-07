use std::fmt::Debug;
use std::net::SocketAddr;
use std::pin::Pin;

use async_std::net::TcpStream;
use async_trait::async_trait;
use deadpool::managed::{Manager, Object, RecycleResult};
use futures::io::{AsyncRead, AsyncWrite};
use futures::task::{Context, Poll};

cfg_if::cfg_if! {
    if #[cfg(feature = "rustls_client")] {
        use async_tls::client::TlsStream;
    } else if #[cfg(feature = "native-tls")] {
        use async_native_tls::TlsStream;
    }
}

use crate::Error;

#[derive(Clone, Debug)]
pub(crate) struct TlsConnection {
    host: String,
    addr: SocketAddr,
}
impl TlsConnection {
    pub(crate) fn new(host: String, addr: SocketAddr) -> Self {
        Self { host, addr }
    }
}

pub(crate) struct TlsConnWrapper {
    conn: Object<TlsStream<TcpStream>, Error>,
}
impl TlsConnWrapper {
    pub(crate) fn new(conn: Object<TlsStream<TcpStream>, Error>) -> Self {
        Self { conn }
    }
}

impl AsyncRead for TlsConnWrapper {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut *self.conn).poll_read(cx, buf)
    }
}

impl AsyncWrite for TlsConnWrapper {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.conn).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.conn).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.conn).poll_close(cx)
    }
}

#[async_trait]
impl Manager<TlsStream<TcpStream>, Error> for TlsConnection {
    async fn create(&self) -> Result<TlsStream<TcpStream>, Error> {
        let raw_stream = async_std::net::TcpStream::connect(self.addr).await?;
        let tls_stream = add_tls(&self.host, raw_stream).await?;
        Ok(tls_stream)
    }

    async fn recycle(&self, conn: &mut TlsStream<TcpStream>) -> RecycleResult<Error> {
        let mut buf = [0; 4];
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        match Pin::new(conn).poll_read(&mut cx, &mut buf) {
            Poll::Ready(Err(error)) => Err(error),
            Poll::Ready(Ok(bytes)) if bytes == 0 => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection appeared to be closed (EoF)",
            )),
            _ => Ok(()),
        }
        .map_err(Error::from)?;
        Ok(())
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "rustls_client")] {
        async fn add_tls(host: &str, stream: TcpStream) -> Result<TlsStream<TcpStream>, std::io::Error> {
            use std::sync::Arc;

            let mut cfg = rustls::ClientConfig::new();
            cfg.dangerous().set_certificate_verifier(Arc::new(NoCertificateVerification {}));
            let connector = async_tls::TlsConnector::from(cfg);
            connector.connect(host, stream).await
        }
    } else if #[cfg(feature = "native-tls")] {
        async fn add_tls(
            host: &str,
            stream: TcpStream,
        ) -> Result<TlsStream<TcpStream>, async_native_tls::Error> {
            async_native_tls::connect(host, stream).await
        }
    }
}

#[cfg(feature = "rustls_client")]
pub struct NoCertificateVerification {}

#[cfg(feature = "rustls_client")]
impl rustls::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _roots: &rustls::RootCertStore,
        _presented_certs: &[rustls::Certificate],
        _dns_name: webpki::DNSNameRef<'_>,
        _ocsp: &[u8],
    ) -> Result<rustls::ServerCertVerified, rustls::TLSError> {
        Ok(rustls::ServerCertVerified::assertion())
    }
}