# relay

## Overview
TURN relay discovery for call media. Fetches short-lived TURN credentials from the Teams `trap/tokens` endpoint, turns them into ICE servers, and probes the relay gateways with a STUN/TURN `Allocate` (over UDP 3478, or over TLS on 443 through a loopback UDP shim) to find a gateway that accepts the credentials before the WebRTC peer in `../media` is built.

## Interface
- `relay/mod.rs`
  - Constants: `TOKENS_URL` (`https://teams.microsoft.com/trap/tokens`), `DEFAULT_RELAY_HOST`, `RELAY_UDP_PORT` (3478). `pub mod allocate; pub mod tls;`.
  - `RelayCredentials {realm, username, password}` (custom `Debug` omits username and password), `RelayGrant {credentials, expires_in}`.
  - `parse_tokens(&str) -> Result<RelayGrant>`, `ice_servers(&RelayGrant, host) -> Vec<IceServer>` (udp:3478, tcp:443 and turns:443 URLs), `resolved_ice_servers(&RelayGrant, SocketAddr)`, `relay_headers`, `relay_hosts(authz region) -> Vec<String>`.
  - Async: `fetch_grant(&Client)`, `fetch_ice_servers(&Client, host)`, `resolve_ice_servers(&Client, host, Option<&str>)` (UDP probe, follows 300 redirects), `resolve_tls_ice_servers(...) -> (Vec<IceServer>, Duration, tls::TlsShim)`.
- `relay/allocate.rs`: `AllocateProbe` (I/O-free TURN Allocate state machine: `new`, `start`, `on_answer`), `ProbeStep {Send, Found}`, `MAX_REDIRECTS` (3), `find_relay(SocketAddr, &RelayCredentials) -> Result<SocketAddr>`.
- `relay/tls.rs`: `RELAY_TLS_PORT` (443), `frame_for_stream`, `StreamDeframer` (`push`, `next_frame`), `TlsRelay`, `client_config()`, `find_relay_tls(...)`, `TlsShim` (`start`, `local_addr`, `turn_url`).

## Invariants
- An empty token list is a protocol error; only the first credential set is probed. A non-2xx response from `trap/tokens` becomes `Error::Status` with the body cut to 400 chars [mod.rs:parse_tokens, fetch_grant].
- Allocate requests always carry REQUESTED-TRANSPORT UDP and a FINGERPRINT; authenticated ones add USERNAME, REALM, NONCE and MESSAGE-INTEGRITY. Answers are ignored unless the transaction id matches [allocate.rs:AllocateProbe::request, on_answer].
- At most 2 challenges (401/438) per server; a 300 needs ALTERNATE-SERVER and fails on a loop or after more than `MAX_REDIRECTS` hops [allocate.rs:MAX_CHALLENGES, on_answer].
- The UDP probe sends up to 3 times (RTO 500 ms, doubling) and ignores replies from other addresses [allocate.rs:find_relay].
- TLS verification is never turned off: rustls with the ring provider and webpki roots, no client auth; connect timeout 5 s, answer timeout 5 s [tls.rs:client_config, connect, ANSWER_TIMEOUT].
- Stream framing: STUN is framed by its length (magic cookie checked), ChannelData is padded to 4 bytes; other first-bit patterns are dropped or error [tls.rs:frame_for_stream, StreamDeframer::next_frame].
- The shim serves at most 8 clients, queues 128 datagrams per stream (overflow dropped) and remembers the last 32 request transaction ids to drop retransmissions [tls.rs:MAX_CLIENTS, QUEUE, SEEN_REQUESTS].

## Side Effects
- HTTPS GET to `trap/tokens` with a Bearer IC3 token [mod.rs:fetch_grant].
- DNS lookups for gateway hosts; ephemeral UDP socket for probes; TCP+TLS connections to port 443 [mod.rs:host_addrs, probe_host, probe_host_tls].
- Allocations are released with a Refresh (LIFETIME 0) after the probe [allocate.rs:AllocateProbe::on_answer].
- `TlsShim::start` binds `127.0.0.1:0` and spawns a router task; `Drop` aborts it and closes every stream [tls.rs:TlsShim].
- `tracing` logs at info, warn, debug and trace levels.

## Verification
- Run: `cargo test -p calling`
