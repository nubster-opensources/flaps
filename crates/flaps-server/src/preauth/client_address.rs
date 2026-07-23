//! The client address a pre-authentication budget layer is keyed on.
//!
//! Only the connection address is ever used. No caller-controlled header is
//! trusted, `X-Forwarded-For` included: a header an attacker can set turns the
//! per-address layer into a per-attacker-choice layer, which is worse than no
//! layer at all.
//!
//! Behind a reverse proxy every request therefore shares one address, and the
//! per-address layer degenerates into a second global layer. That is an
//! accepted loss of granularity, documented in the operations guide: an
//! operator who wants per-client granularity must enforce it at the proxy.

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

/// The address a request was received from, when it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAddress {
    /// The connection address reported by the transport.
    Known(IpAddr),
    /// No connection address is available.
    ///
    /// This happens when the router is driven directly (tests, an embedding
    /// application) rather than served over a TCP listener carrying connection
    /// information. Such requests are not exempt from the budget: they all
    /// share one bucket, so a missing address degrades the layer instead of
    /// disabling it.
    Unknown,
}

impl ClientAddress {
    /// Returns the identifier this address is bucketed under.
    ///
    /// The port is deliberately excluded: keying on it would hand every new
    /// connection a fresh budget, which is the exact rotation this layer
    /// exists to stop.
    #[must_use]
    pub fn bucket_key(&self) -> String {
        match self {
            Self::Known(address) => address.to_string(),
            Self::Unknown => "unknown".to_owned(),
        }
    }
}

impl From<SocketAddr> for ClientAddress {
    fn from(address: SocketAddr) -> Self {
        Self::Known(address.ip())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ClientAddress {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(Self::Unknown, |ConnectInfo(address)| Self::from(*address)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn an_absent_connection_address_is_a_single_shared_identity() {
        assert_eq!(ClientAddress::Unknown.bucket_key(), "unknown");
    }

    #[test]
    fn a_known_address_keys_on_the_address_alone() {
        let address = ClientAddress::Known(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)));
        assert_eq!(address.bucket_key(), "203.0.113.7");
    }

    #[test]
    fn the_port_is_not_part_of_the_key() {
        // Keying on the port would give every new connection a fresh budget,
        // which is the exact rotation the layer exists to stop.
        use std::net::SocketAddr;
        let first: SocketAddr = "203.0.113.7:40000".parse().expect("socket address");
        let second: SocketAddr = "203.0.113.7:40001".parse().expect("socket address");
        assert_eq!(
            ClientAddress::from(first).bucket_key(),
            ClientAddress::from(second).bucket_key()
        );
    }

    #[tokio::test]
    async fn the_extractor_reports_the_connection_address_when_present() {
        use axum::extract::FromRequestParts;

        let socket_address: SocketAddr = "203.0.113.7:40000".parse().expect("socket address");
        let mut parts = axum::http::Request::new(()).into_parts().0;
        parts.extensions.insert(ConnectInfo(socket_address));

        let extracted = ClientAddress::from_request_parts(&mut parts, &())
            .await
            .expect("the extractor is infallible");

        assert_eq!(
            extracted,
            ClientAddress::Known(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)))
        );
    }

    #[tokio::test]
    async fn the_extractor_falls_back_to_unknown_when_no_connection_address_is_recorded() {
        use axum::extract::FromRequestParts;

        let mut parts = axum::http::Request::new(()).into_parts().0;

        let extracted = ClientAddress::from_request_parts(&mut parts, &())
            .await
            .expect("the extractor is infallible");

        assert_eq!(extracted, ClientAddress::Unknown);
    }
}
