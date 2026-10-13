//! Which of this Mac's addresses a phone on the same Wi-Fi could reach.
//!
//! ```text
//! getifaddrs ──▶ up and running? ──▶ not loopback? ──▶ not a tunnel?
//!                                                          │
//!                                                          ▼
//!                                              a private IPv4 address?
//!                                                          │
//!                                                          ▼
//!                                                    offer it to the
//!                                                    developer to pick
//! ```
//!
//! The interfaces are read when they are asked for and never kept: a Mac
//! moves between networks, and an address list that was true five minutes ago
//! is a list of addresses that no longer bind. Nothing here is written
//! anywhere.
//!
//! # What is left out, and why
//!
//! **Loopback**, because `127.0.0.1` is where the proxy already is and no
//! phone can reach it. **An interface that is not up and running**, because
//! binding to it either fails or produces a listener nothing can connect to.
//! **A point-to-point interface** — a VPN's `utun`, a `ppp` link — because
//! the other end of it is not the Wi-Fi the phone is on. **A link-local
//! address** (`169.254.x.x`), because it is what an interface has when DHCP
//! did not answer.
//!
//! **And any address that is not private.** This is the one filter that is
//! about safety rather than about usefulness: a Mac with a publicly routable
//! address on an interface is a Mac where binding the proxy to that address
//! would put an open HTTP proxy on the Internet. ProxyLab will not offer
//! that, so what it offers is RFC 1918 — `10.0.0.0/8`, `172.16.0.0/12`,
//! `192.168.0.0/16` — which is what a home or office LAN hands out.
//!
//! # IPv4 only
//!
//! An iPhone's manual Wi-Fi proxy setting takes a server address and a port,
//! and the address a developer reads off the Mac and types into the phone is
//! an IPv4 one. An IPv6 address on a LAN is usually link-local, which needs a
//! zone identifier that means something different on each machine, so there
//! is nothing useful to type. Nothing else in ProxyLab is IPv4-only; this
//! list is.

use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr};

use proxy_ipc::LanAddress;

/// Every address of this Mac that a LAN listener could usefully be bound to.
///
/// Read from the operating system each time, in the order the interfaces
/// come back in, with duplicates removed. An empty list is an ordinary
/// answer: a Mac with no network, or one on a network ProxyLab will not
/// offer, has no address to pick.
pub fn usable() -> Vec<LanAddress> {
    let mut found: Vec<LanAddress> = Vec::new();

    for (interface, flags, address) in interfaces() {
        if !usable_interface(flags) || !usable_address(address) {
            continue;
        }

        let candidate = LanAddress {
            interface,
            address: address.to_string(),
        };
        if !found.contains(&candidate) {
            found.push(candidate);
        }
    }

    found
}

/// Whether `address` is one this Mac is offering right now.
///
/// Asked again at the moment of binding rather than trusted from an earlier
/// answer: the choice a client sends was made from a list, and the list may
/// have been made before the Mac changed networks.
pub fn is_usable(address: Ipv4Addr) -> bool {
    let address = address.to_string();

    usable().iter().any(|usable| usable.address == address)
}

/// Whether an interface with these flags is one to offer.
fn usable_interface(flags: u32) -> bool {
    let up = flags & (libc::IFF_UP as u32) != 0;
    let running = flags & (libc::IFF_RUNNING as u32) != 0;
    let loopback = flags & (libc::IFF_LOOPBACK as u32) != 0;
    let tunnel = flags & (libc::IFF_POINTOPOINT as u32) != 0;

    up && running && !loopback && !tunnel
}

/// Whether an address is one to offer.
///
/// Private and nothing else: see the module documentation for why a routable
/// address is refused rather than merely warned about.
fn usable_address(address: Ipv4Addr) -> bool {
    address.is_private()
        && !address.is_link_local()
        && !address.is_loopback()
        && !address.is_broadcast()
        && !address.is_unspecified()
}

/// Every IPv4 interface address the operating system reports, with its flags.
///
/// The one place this crate touches `getifaddrs`. Everything above it works
/// on `(name, flags, address)` and is testable without a network.
fn interfaces() -> Vec<(String, u32, Ipv4Addr)> {
    let mut list = std::ptr::null_mut();

    // SAFETY: `getifaddrs` writes one owned linked list through `list` and
    // reports whether it did. Nothing is read unless it returned zero, and
    // the list is freed once, below, with no pointer into it kept: every
    // name and address is copied out.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        proxy_diagnostics::warn(
            "lan",
            "discover",
            "configuration",
            "failed to read this machine's network interfaces",
        );
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut entry = list;

    while !entry.is_null() {
        // SAFETY: `entry` is non-null and came from the list `getifaddrs`
        // produced, so it points at a live `ifaddrs` until `freeifaddrs`.
        let current = unsafe { &*entry };
        entry = current.ifa_next;

        if current.ifa_name.is_null() || current.ifa_addr.is_null() {
            continue;
        }

        // SAFETY: `ifa_addr` is non-null and points at a `sockaddr` whose
        // family says which larger structure it really is. Only `sa_family`
        // is read here, which every one of them begins with.
        let family = unsafe { (*current.ifa_addr).sa_family };
        if i32::from(family) != libc::AF_INET {
            continue;
        }

        // SAFETY: the family said `AF_INET`, so the `sockaddr` is the first
        // field of a `sockaddr_in`, which is what is read. The value is
        // copied out rather than borrowed.
        let address = unsafe { *current.ifa_addr.cast::<libc::sockaddr_in>() };

        // SAFETY: `ifa_name` is non-null and is the interface's own
        // NUL-terminated name. It is copied into a `String` here and the
        // pointer is not kept.
        let name = unsafe { CStr::from_ptr(current.ifa_name) };
        let Ok(name) = name.to_str() else { continue };

        found.push((
            name.to_owned(),
            current.ifa_flags,
            Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)),
        ));
    }

    // SAFETY: `list` is the list `getifaddrs` produced, freed exactly once,
    // and nothing above keeps a pointer into it.
    unsafe { libc::freeifaddrs(list) };

    found
}

/// Reads text as an address a LAN listener could be bound to.
///
/// IPv4 only, for the reason the module documentation gives, so an IPv6
/// address is refused here rather than offered and then failing to bind.
pub fn parse(address: &str) -> Option<Ipv4Addr> {
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => Some(address),
        Ok(IpAddr::V6(_)) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP_AND_RUNNING: u32 = libc::IFF_UP as u32 | libc::IFF_RUNNING as u32;

    #[test]
    fn an_interface_that_is_up_and_running_is_offered() {
        assert!(usable_interface(UP_AND_RUNNING));
    }

    #[test]
    fn an_interface_that_is_down_is_not() {
        assert!(!usable_interface(libc::IFF_RUNNING as u32));
        assert!(!usable_interface(libc::IFF_UP as u32));
        assert!(!usable_interface(0));
    }

    #[test]
    fn loopback_is_not_offered() {
        assert!(!usable_interface(
            UP_AND_RUNNING | libc::IFF_LOOPBACK as u32
        ));
    }

    #[test]
    fn a_tunnel_is_not_offered() {
        assert!(!usable_interface(
            UP_AND_RUNNING | libc::IFF_POINTOPOINT as u32
        ));
    }

    #[test]
    fn a_private_address_is_offered() {
        for address in ["192.168.1.42", "10.0.0.7", "172.16.5.9"] {
            assert!(
                usable_address(address.parse().expect("an address")),
                "{address}"
            );
        }
    }

    #[test]
    fn a_routable_address_is_not_offered() {
        for address in ["93.184.216.34", "8.8.8.8", "172.32.0.1"] {
            assert!(
                !usable_address(address.parse().expect("an address")),
                "{address}"
            );
        }
    }

    #[test]
    fn loopback_and_link_local_addresses_are_not_offered() {
        assert!(!usable_address("127.0.0.1".parse().expect("an address")));
        assert!(!usable_address("169.254.3.4".parse().expect("an address")));
        assert!(!usable_address("0.0.0.0".parse().expect("an address")));
        assert!(!usable_address(
            "255.255.255.255".parse().expect("an address")
        ));
    }

    #[test]
    fn an_ipv4_address_is_read() {
        assert_eq!(parse("192.168.1.42"), Some(Ipv4Addr::new(192, 168, 1, 42)));
    }

    #[test]
    fn an_ipv6_address_is_not_one_this_list_offers() {
        assert_eq!(parse("fe80::1"), None);
        assert_eq!(parse("::1"), None);
    }

    #[test]
    fn something_that_is_not_an_address_is_refused() {
        for text in ["", "example.com", "192.168.1.42:9090", "not an address"] {
            assert_eq!(parse(text), None, "{text}");
        }
    }

    #[test]
    fn what_the_machine_offers_is_never_loopback_and_always_readable() {
        for address in usable() {
            let parsed = parse(&address.address).expect("an address this module produced");

            assert!(!parsed.is_loopback(), "{parsed}");
            assert!(parsed.is_private(), "{parsed}");
            assert!(!address.interface.is_empty());
        }
    }

    #[test]
    fn an_address_this_machine_does_not_have_is_not_usable() {
        // Private, so it passes every filter but the last one, and reserved
        // by RFC 5737 style convention inside a range no interface here will
        // be holding: what is under test is that the answer comes from the
        // machine rather than from the shape of the address.
        assert!(!is_usable(Ipv4Addr::new(10, 255, 255, 254)));
    }

    #[test]
    fn every_address_the_machine_offers_is_usable() {
        for address in usable() {
            assert!(is_usable(parse(&address.address).expect("an address")));
        }
    }
}
