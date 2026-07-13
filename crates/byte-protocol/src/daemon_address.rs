//! Daemon WebSocket address discovery and validation.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

/// Errors that can occur when parsing or validating a daemon address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DaemonAddressError {
    /// The address string is not a valid socket address.
    #[error("invalid daemon address: {0}")]
    InvalidAddress(String),
    /// The address is not a loopback address.
    #[error("daemon address must be 127.0.0.1 or localhost, got: {0}")]
    NotLocalHost(String),
}

/// A validated loopback WebSocket address for the local daemon.
///
/// Only `127.0.0.1` and `localhost` are accepted; all other addresses are
/// rejected to keep the daemon local-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DaemonAddress {
    inner: SocketAddr,
}

impl DaemonAddress {
    /// Default daemon WebSocket address.
    pub const DEFAULT: Self = Self {
        inner: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8787),
    };

    /// Create a validated address from a socket address.
    ///
    /// # Errors
    ///
    /// Returns an error if the address is not loopback.
    pub fn from_socket_addr(addr: SocketAddr) -> Result<Self, DaemonAddressError> {
        if !addr.ip().is_loopback() {
            return Err(DaemonAddressError::NotLocalHost(addr.to_string()));
        }
        Ok(Self { inner: addr })
    }

    /// Return the underlying socket address.
    #[must_use]
    pub const fn socket_addr(&self) -> SocketAddr {
        self.inner
    }

    /// Return the WebSocket URL for this address (`ws://127.0.0.1:8787`).
    #[must_use]
    pub fn websocket_url(&self) -> String {
        format!("ws://{}/", self.inner)
    }
}

impl FromStr for DaemonAddress {
    type Err = DaemonAddressError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Normalize "localhost" to 127.0.0.1 so `localhost:8787" parses as a
        // socket address. Reject everything else early.
        let normalized = if s.starts_with("localhost:") || s == "localhost" {
            s.replacen("localhost", "127.0.0.1", 1)
        } else {
            s.to_owned()
        };

        let addr = SocketAddr::from_str(&normalized)
            .map_err(|_| DaemonAddressError::InvalidAddress(s.to_owned()))?;
        Self::from_socket_addr(addr)
    }
}

impl TryFrom<String> for DaemonAddress {
    type Error = DaemonAddressError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<DaemonAddress> for String {
    fn from(value: DaemonAddress) -> Self {
        value.to_string()
    }
}

impl fmt::Display for DaemonAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn accepts_localhost_and_loopback() {
        assert!("127.0.0.1:8787".parse::<DaemonAddress>().is_ok());
        assert!("localhost:8787".parse::<DaemonAddress>().is_ok());
        assert_eq!(
            "localhost:8787".parse::<DaemonAddress>().unwrap().to_string(),
            "127.0.0.1:8787"
        );
    }

    #[test]
    fn rejects_non_loopback_addresses() {
        assert!("0.0.0.0:8787".parse::<DaemonAddress>().is_err());
        assert!("192.168.1.1:8787".parse::<DaemonAddress>().is_err());
        assert!("example.com:8787".parse::<DaemonAddress>().is_err());
    }

    #[test]
    fn rejects_invalid_socket_addresses() {
        assert!("not-an-address".parse::<DaemonAddress>().is_err());
        assert!("127.0.0.1".parse::<DaemonAddress>().is_err());
    }

    #[test]
    fn produces_ws_url() {
        let addr = DaemonAddress::from_str("127.0.0.1:8787").unwrap();
        assert_eq!(addr.websocket_url(), "ws://127.0.0.1:8787/");
    }

    #[test]
    fn roundtrips_through_string() {
        let addr: DaemonAddress = "127.0.0.1:8787".parse().unwrap();
        let s: String = addr.into();
        assert_eq!(s, "127.0.0.1:8787");
        assert_eq!(DaemonAddress::from_str(&s).unwrap(), addr);
    }
}
