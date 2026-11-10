//! Client IP resolution, honouring `X-Forwarded-For` only from trusted proxies.

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::request::Parts,
};

use crate::{
    config::{canonical_ip, IpNet},
    error::AppError,
    state::AppState,
};

/// The address of the end client. Behind a reverse proxy listed in
/// `MUZIK_TRUSTED_PROXIES`, it is the right-most `X-Forwarded-For` entry that
/// is not itself a trusted proxy; otherwise it is the TCP peer. A client can
/// therefore never spoof its address by sending its own header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl std::fmt::Display for ClientIp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub fn resolve(peer: IpAddr, forwarded_for: Option<&str>, trusted: &[IpNet]) -> IpAddr {
    let peer = canonical_ip(peer);
    let is_trusted = |ip: IpAddr| trusted.iter().any(|n| n.contains(ip));
    if !is_trusted(peer) {
        return peer;
    }
    let Some(xff) = forwarded_for else {
        return peer;
    };
    let mut client = peer;
    for hop in xff.rsplit(',') {
        let Ok(ip) = hop.trim().parse::<IpAddr>() else {
            // Garbage in the chain: stop at the last address we could trust.
            break;
        };
        client = canonical_ip(ip);
        if !is_trusted(client) {
            break;
        }
    }
    client
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .ok_or_else(|| AppError::Internal("missing peer address".into()))?;
        // Several X-Forwarded-For headers are joined in order.
        let xff: Vec<&str> = parts
            .headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        let xff = (!xff.is_empty()).then(|| xff.join(","));
        Ok(Self(resolve(
            peer,
            xff.as_deref(),
            &state.config.trusted_proxies,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ignores_header_from_untrusted_peer() {
        let trusted = [IpNet::parse("10.0.0.0/8").unwrap()];
        assert_eq!(
            resolve(ip("203.0.113.5"), Some("1.2.3.4"), &trusted),
            ip("203.0.113.5")
        );
        assert_eq!(
            resolve(ip("10.0.0.2"), Some("1.2.3.4"), &[]),
            ip("10.0.0.2")
        );
    }

    #[test]
    fn takes_rightmost_untrusted_hop() {
        let trusted = [IpNet::parse("10.0.0.0/8").unwrap()];
        // Client-supplied spoofed entry on the left is ignored.
        assert_eq!(
            resolve(
                ip("10.0.0.2"),
                Some("6.6.6.6, 198.51.100.7, 10.0.0.3"),
                &trusted
            ),
            ip("198.51.100.7")
        );
        assert_eq!(resolve(ip("10.0.0.2"), None, &trusted), ip("10.0.0.2"));
        assert_eq!(
            resolve(ip("::ffff:10.0.0.2"), Some("198.51.100.7"), &trusted),
            ip("198.51.100.7")
        );
        assert_eq!(
            resolve(ip("10.0.0.2"), Some("bogus, 10.0.0.9"), &trusted),
            ip("10.0.0.9")
        );
    }
}
