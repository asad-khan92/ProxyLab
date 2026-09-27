//! What counts as this proxy, for the bridge's two loop checks.
//!
//! A flow the provider hands over is refused when it is the proxy talking to
//! itself — either because it came from this process, or because it is going
//! to one of this process's own listeners. Both facts are known when the
//! proxy starts and do not change while it runs, so they are captured once
//! here and consulted per flow.

use std::net::SocketAddr;

/// This process, and the ports it listens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Own {
    pid: u32,
    ports: Vec<u16>,
}

impl Own {
    /// The running process, listening on `listeners`.
    ///
    /// `listeners` is every address this proxy accepts connections on that a
    /// flow could be pointed back at: the explicit proxy and the bridge.
    pub fn this_process(listeners: &[SocketAddr]) -> Self {
        Self::new(std::process::id(), listeners)
    }

    /// A process of the caller's choosing, for a test that has to be some
    /// other process than the one it is running in.
    pub fn new(pid: u32, listeners: &[SocketAddr]) -> Self {
        Self {
            pid,
            ports: listeners.iter().map(SocketAddr::port).collect(),
        }
    }

    /// Whether a flow whose `CONNECT` named `source` came from this process.
    ///
    /// Anything that is not exactly a decimal process identifier is not this
    /// process: a malformed header is a flow that was not ours, and it is
    /// carried rather than refused.
    pub fn is_source(&self, source: &str) -> bool {
        source.bytes().all(|byte| byte.is_ascii_digit())
            && source.parse::<u32>().ok() == Some(self.pid)
    }

    /// Whether connecting to `destination` would be connecting to this proxy.
    ///
    /// A listener here is bound to loopback, so that is where it can be
    /// reached — and so can the unspecified address, which a connection
    /// treats as this machine. A loopback address on some other port is
    /// something else on this Mac, and is not refused.
    pub fn is_destination(&self, destination: &SocketAddr) -> bool {
        let ip = destination.ip();

        (ip.is_loopback() || ip.is_unspecified()) && self.ports.contains(&destination.port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> Own {
        Own::new(
            4242,
            &[
                "127.0.0.1:9090".parse().expect("address"),
                "127.0.0.1:9091".parse().expect("address"),
            ],
        )
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().expect("socket address")
    }

    #[test]
    fn a_flow_from_this_process_is_its_own() {
        assert!(own().is_source("4242"));
    }

    #[test]
    fn a_flow_from_any_other_process_is_not() {
        for source in ["4243", "1", "0", ""] {
            assert!(!own().is_source(source), "{source:?}");
        }
    }

    #[test]
    fn a_header_that_is_not_a_process_identifier_is_not_this_process() {
        for source in ["+4242", " 4242", "4242 ", "0x1092", "4242.0", "-4242"] {
            assert!(!own().is_source(source), "{source:?}");
        }
    }

    #[test]
    fn its_own_listeners_are_its_own_destinations() {
        assert!(own().is_destination(&addr("127.0.0.1:9090")));
        assert!(own().is_destination(&addr("127.0.0.1:9091")));
        assert!(own().is_destination(&addr("127.3.2.1:9091")));
        assert!(own().is_destination(&addr("[::1]:9090")));
    }

    #[test]
    fn the_unspecified_address_reaches_this_machine_too() {
        assert!(own().is_destination(&addr("0.0.0.0:9091")));
        assert!(own().is_destination(&addr("[::]:9090")));
    }

    #[test]
    fn something_else_on_this_mac_is_not_this_proxy() {
        assert!(!own().is_destination(&addr("127.0.0.1:8080")));
    }

    #[test]
    fn the_same_port_somewhere_else_is_not_this_proxy() {
        assert!(!own().is_destination(&addr("93.184.215.14:9090")));
        assert!(!own().is_destination(&addr("192.168.1.42:9091")));
    }
}
