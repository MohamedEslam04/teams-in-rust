//! TURN over TLS (`turns:<gateway>:443`) for networks that block UDP, through a loopback shim.
//!
//! webrtc-rs 0.21.1 only speaks TURN over UDP: it skips `turns:` and `transport=tcp` URLs. So
//! IronTeams gives it a TURN server on loopback, `turn:127.0.0.1:<port>?transport=udp`, and
//! [`TlsShim`] carries what arrives there over TLS to the relay's port 443 and back. The relayed
//! transport stays UDP (REQUESTED-TRANSPORT 17); only the client-to-relay leg is TLS, as RFC 8656
//! §3.1 allows. On the stream, STUN messages are delimited by their length field and ChannelData
//! is padded to four bytes (RFC 8656 §12.5); [`frame_for_stream`] and [`StreamDeframer`] do that.
//!
//! An allocation belongs to one client 5-tuple, so the shim opens one TLS stream per webrtc-rs
//! socket that talks to it (webrtc-rs runs one TURN client per local interface, and a call can
//! build a fresh peer connection on renegotiation). A stream is opened on its socket's first
//! datagram; [`TlsShim::start`] connects once beforehand, so a relay that is unreachable over TLS
//! (or shows a bad certificate) fails before the call is placed.
//!
//! Certificates are checked against the web PKI roots (`webpki-roots`) for the gateway's name,
//! also on an alternate the gateway redirects to: the redirect arrived over a TLS session already
//! verified for that name, and the relays share one certificate (`*.az.relay.teams.cloud.microsoft`
//! among its names; seen live on every gateway, 2026-11-18). Verification is never turned off.

use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::{TlsConnector, rustls};

use super::RelayCredentials;
use super::allocate::{AllocateProbe, ProbeStep};
use crate::error::{Error, Result};

/// The relay gateways' TURN-over-TLS port.
pub const RELAY_TLS_PORT: u16 = 443;
const STUN_HEADER: usize = 20;
const CHANNEL_HEADER: usize = 4;
const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
/// TCP and TLS connect, together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the probe waits for an answer; TLS is reliable, so nothing is retransmitted.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
/// webrtc-rs sockets served at once (one per local interface and peer connection).
const MAX_CLIENTS: usize = 8;
/// Datagrams queued for one stream while it connects or the relay is slow; beyond it they are
/// dropped, as a congested UDP path would.
const QUEUE: usize = 128;
/// Request transaction ids remembered per stream to drop webrtc-rs's retransmissions.
const SEEN_REQUESTS: usize = 32;

/// The stream framing of one datagram from webrtc-rs: a STUN message as it is, ChannelData
/// padded to a multiple of four bytes. `None` for anything else (it is dropped).
pub fn frame_for_stream(datagram: &[u8]) -> Option<Vec<u8>> {
    let first = *datagram.first()?;
    match first >> 6 {
        0 => {
            let len = STUN_HEADER
                + usize::from(u16::from_be_bytes([*datagram.get(2)?, *datagram.get(3)?]));
            (datagram.len() >= len).then(|| datagram[..len].to_vec())
        }
        1 => {
            let len = CHANNEL_HEADER
                + usize::from(u16::from_be_bytes([*datagram.get(2)?, *datagram.get(3)?]));
            if datagram.len() < len {
                return None;
            }
            let mut framed = datagram[..len].to_vec();
            framed.resize(len.next_multiple_of(4), 0);
            Some(framed)
        }
        _ => None,
    }
}

/// Splits the bytes read from a TURN stream back into datagrams (ChannelData without its
/// padding). The buffer holds at most one frame plus one read, so it stays bounded.
#[derive(Debug, Default)]
pub struct StreamDeframer {
    buf: Vec<u8>,
}

impl StreamDeframer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole datagram, `Ok(None)` until one is complete. An error means the stream
    /// lost its framing and must be closed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        if self.buf.len() < CHANNEL_HEADER {
            return Ok(None);
        }
        let len = usize::from(u16::from_be_bytes([self.buf[2], self.buf[3]]));
        let (datagram, on_stream) = match self.buf[0] >> 6 {
            0 => {
                if self.buf.len() >= 8 && self.buf[4..8] != MAGIC_COOKIE || len % 4 != 0 {
                    return Err(Error::Protocol("TURN stream: not a STUN message".into()));
                }
                (STUN_HEADER + len, STUN_HEADER + len)
            }
            1 => (
                CHANNEL_HEADER + len,
                (CHANNEL_HEADER + len).next_multiple_of(4),
            ),
            _ => return Err(Error::Protocol("TURN stream lost its framing".into())),
        };
        if self.buf.len() < on_stream {
            return Ok(None);
        }
        let frame = self.buf[..datagram].to_vec();
        self.buf.drain(..on_stream);
        Ok(Some(frame))
    }
}

/// Where a relay is reached over TLS: the address to connect to and the name its certificate
/// must carry (also the SNI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsRelay {
    pub addr: SocketAddr,
    pub server_name: String,
}

/// TLS client settings trusting the web PKI roots. The ring provider is named explicitly,
/// so this does not depend on the process default being installed.
pub fn client_config() -> Arc<ClientConfig> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring supports TLS 1.2 and 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

async fn connect(relay: &TlsRelay, config: &Arc<ClientConfig>) -> Result<TlsStream<TcpStream>> {
    let name = ServerName::try_from(relay.server_name.clone())
        .map_err(|e| Error::Protocol(format!("TURN TLS name {}: {e}", relay.server_name)))?;
    let connector = TlsConnector::from(Arc::clone(config));
    let handshake = async {
        let tcp = TcpStream::connect(relay.addr).await?;
        tcp.set_nodelay(true)?;
        connector.connect(name, tcp).await
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, handshake).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(Error::Protocol(format!(
            "TURN over TLS to {}: {e}",
            relay.addr
        ))),
        Err(_) => Err(Error::Protocol(format!(
            "TURN over TLS to {}: no connection within {CONNECT_TIMEOUT:?}",
            relay.addr
        ))),
    }
}

/// Reads from `stream` until the probe has its answer.
async fn next_step(
    probe: &mut AllocateProbe,
    stream: &mut TlsStream<TcpStream>,
    deframer: &mut StreamDeframer,
) -> Result<ProbeStep> {
    let mut buf = vec![0u8; 4096];
    loop {
        while let Some(frame) = deframer.next_frame()? {
            if let Some(step) = probe.on_answer(&frame)? {
                return Ok(step);
            }
        }
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| Error::Protocol(format!("TURN over TLS: {e}")))?;
        if n == 0 {
            return Err(Error::Protocol("TURN relay closed the TLS stream".into()));
        }
        deframer.push(&buf[..n]);
    }
}

/// [`super::allocate::find_relay`] over TLS: asks `server` (whose certificate must name
/// `server_name`) for an allocation, following its redirects, and returns the relay that
/// allocated. An alternate gets a new TLS stream, checked for the same name. The probe's
/// allocation is released before this returns.
pub async fn find_relay_tls(
    server: SocketAddr,
    server_name: &str,
    credentials: &RelayCredentials,
    config: Arc<ClientConfig>,
) -> Result<TlsRelay> {
    let mut probe = AllocateProbe::new(server, credentials);
    let mut step = probe.start()?;
    let mut stream: Option<(SocketAddr, TlsStream<TcpStream>, StreamDeframer)> = None;
    loop {
        let (to, request) = match step {
            ProbeStep::Found { server, release } => {
                if let Some((_, s, _)) = stream.as_mut() {
                    let _ = s.write_all(&release).await;
                    let _ = s.shutdown().await;
                }
                return Ok(TlsRelay {
                    addr: server,
                    server_name: server_name.to_owned(),
                });
            }
            ProbeStep::Send { to, datagram } => (to, datagram),
        };
        if !matches!(&stream, Some((at, ..)) if *at == to) {
            tracing::debug!(%to, "TURN over TLS probe: connecting");
            let relay = TlsRelay {
                addr: to,
                server_name: server_name.to_owned(),
            };
            stream = Some((
                to,
                connect(&relay, &config).await?,
                StreamDeframer::default(),
            ));
        }
        let Some((_, s, deframer)) = stream.as_mut() else {
            unreachable!("connected above")
        };
        s.write_all(&request)
            .await
            .map_err(|e| Error::Protocol(format!("TURN over TLS to {to}: {e}")))?;
        step = tokio::time::timeout(ANSWER_TIMEOUT, next_step(&mut probe, s, deframer))
            .await
            .map_err(|_| Error::Protocol(format!("TURN relay {to} did not answer over TLS")))??;
    }
}

/// A TURN server on loopback for webrtc-rs that relays over TLS. Dropping it closes every
/// stream, which ends their allocations.
#[derive(Debug)]
pub struct TlsShim {
    local: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for TlsShim {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TlsShim {
    /// Checks that `relay` takes a TLS connection with a valid certificate, then listens on
    /// `127.0.0.1`.
    pub async fn start(relay: TlsRelay, config: Arc<ClientConfig>) -> Result<Self> {
        let mut check = connect(&relay, &config).await?;
        let _ = check.shutdown().await;
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| Error::Protocol(format!("TURN shim socket: {e}")))?;
        let local = udp
            .local_addr()
            .map_err(|e| Error::Protocol(format!("TURN shim socket: {e}")))?;
        let task = tokio::spawn(route(Arc::new(udp), relay, config));
        Ok(Self { local, task })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// The URL to give webrtc-rs.
    pub fn turn_url(&self) -> String {
        format!("turn:{}?transport=udp", self.local)
    }
}

/// Hands each datagram to its client's stream, opening one for a new client.
async fn route(udp: Arc<UdpSocket>, relay: TlsRelay, config: Arc<ClientConfig>) {
    // A client whose stream ended keeps its (closed) entry: it is not given a new stream, since
    // its allocation died with the old one, and it counts against MAX_CLIENTS.
    let mut clients: HashMap<SocketAddr, mpsc::Sender<Vec<u8>>> = HashMap::new();
    // Dropped with this task, which aborts every stream.
    let mut streams = JoinSet::new();
    let mut buf = vec![0u8; usize::from(u16::MAX)];
    loop {
        let (n, from) = match udp.recv_from(&mut buf).await {
            Ok(received) => received,
            Err(e) => {
                tracing::warn!("TURN shim socket failed: {e}");
                return;
            }
        };
        while streams.try_join_next().is_some() {}
        let tx = match clients.get(&from) {
            Some(tx) => tx,
            None if clients.len() >= MAX_CLIENTS => {
                tracing::warn!(%from, "TURN shim: too many clients, datagram dropped");
                continue;
            }
            None => {
                let (tx, rx) = mpsc::channel(QUEUE);
                streams.spawn(serve(
                    from,
                    rx,
                    Arc::clone(&udp),
                    relay.clone(),
                    Arc::clone(&config),
                ));
                clients.entry(from).or_insert(tx)
            }
        };
        if let Some(frame) = frame_for_stream(&buf[..n])
            && let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(frame)
        {
            tracing::trace!(%from, "TURN shim: stream backed up, datagram dropped");
        }
    }
}

/// The transaction id of a STUN request (not a response, an indication or ChannelData).
fn request_id(frame: &[u8]) -> Option<[u8; 12]> {
    let request = frame.len() >= STUN_HEADER
        && frame[0] >> 6 == 0
        && frame[0] & 0x01 == 0
        && frame[1] & 0x10 == 0;
    request
        .then(|| frame[8..STUN_HEADER].try_into().ok())
        .flatten()
}

/// One client's TLS stream: its datagrams up, the relay's frames back down to it.
async fn serve(
    client: SocketAddr,
    mut rx: mpsc::Receiver<Vec<u8>>,
    udp: Arc<UdpSocket>,
    relay: TlsRelay,
    config: Arc<ClientConfig>,
) {
    let stream = match connect(&relay, &config).await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(%client, "TURN shim: {e}");
            return;
        }
    };
    tracing::debug!(%client, "TURN shim: TLS stream open");
    let (mut read, mut write) = tokio::io::split(stream);
    let up = async {
        // TLS is reliable: a request webrtc-rs retransmits went through already.
        let mut seen: VecDeque<[u8; 12]> = VecDeque::with_capacity(SEEN_REQUESTS);
        while let Some(frame) = rx.recv().await {
            if let Some(tid) = request_id(&frame) {
                if seen.contains(&tid) {
                    continue;
                }
                if seen.len() == SEEN_REQUESTS {
                    seen.pop_front();
                }
                seen.push_back(tid);
            }
            if let Err(e) = write.write_all(&frame).await {
                return format!("write failed: {e}");
            }
            if let Err(e) = write.flush().await {
                return format!("write failed: {e}");
            }
        }
        "the shim closed".to_owned()
    };
    let down = async {
        let mut deframer = StreamDeframer::default();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match read.read(&mut buf).await {
                Ok(0) => return "the relay closed it".to_owned(),
                Ok(n) => n,
                Err(e) => return format!("read failed: {e}"),
            };
            deframer.push(&buf[..n]);
            loop {
                match deframer.next_frame() {
                    Ok(Some(datagram)) => {
                        let _ = udp.send_to(&datagram, client).await;
                    }
                    Ok(None) => break,
                    Err(e) => return e.to_string(),
                }
            }
        }
    };
    let reason = tokio::select! {
        r = up => r,
        r = down => r,
    };
    tracing::warn!(%client, "TURN over TLS stream ended: {reason}");
}
