//! Putting the proxy on the local network, so a real iPhone can reach it.
//!
//! ```text
//! iPhone
//!    │  Wi-Fi, with a manual HTTP proxy set to
//!    ▼  <this Mac's LAN address>:<the proxy's port>
//! LAN listener ──▶ the pipeline the loopback listener already uses
//! ```
//!
//! An iPhone that is not a simulator cannot be configured from this Mac and
//! does not need to be: it is a device on the same Wi-Fi, and a Wi-Fi network
//! has a manual proxy setting. What ProxyLab has to do is be reachable at an
//! address the phone can be pointed at, and stop being reachable when it is
//! told to. That is all this module is.
//!
//! [`discovery`] works out which of this Mac's addresses are worth offering.
//! [`exposure`] binds one of them, and closes it again.
//!
//! # It is off, and it is explicit
//!
//! ProxyLab listens on `127.0.0.1` and nothing else until a person turns this
//! on and picks an address. There is no setting that persists it, nothing
//! that turns it on because a phone appeared, and nothing that binds
//! `0.0.0.0` — see [`exposure`].
//!
//! # What it is not
//!
//! It is not a device manager, a second networking stack, or a special path
//! for phone traffic. Nothing past the listener knows a request came from a
//! phone: an iPhone's request is evaluated against the same rules, carried by
//! the same upstream connections, intercepted by the same CA, captured into
//! the same store, and published on the same event stream as a request from a
//! browser on this Mac.
//!
//! It also touches nothing outside ProxyLab. The macOS system proxy settings,
//! the firewall, and the keychain are all left exactly as they are — turning
//! this on adds a listening socket, and that is the whole of what changes on
//! this machine.

pub mod discovery;
pub mod exposure;

pub use exposure::LanExposure;
