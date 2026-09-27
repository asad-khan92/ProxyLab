//! HTTP/1.1 proxying: forwarded requests and intercepted HTTPS connections.
//!
//! This layer sits between the TCP listener and the server the client is
//! trying to reach. The listener knows nothing about HTTP; everything from
//! reading a client's request to writing back an answer lives here.
//!
//! [`connection`] speaks HTTP/1.1 on an accepted socket and [`service`]
//! decides what the client gets back. From there a request takes one of two
//! routes.
//!
//! A forwarded request, which carries an absolute-form target:
//!
//! 1. [`target`] — work out where the absolute-form request target points.
//! 2. [`headers`] — drop the headers that must not cross the proxy.
//! 3. [`upstream`] — connect to the origin and exchange messages with it.
//!
//! A `CONNECT` request, which carries an authority-form target:
//!
//! 1. [`connect_target`] — work out which host and port is being asked for.
//! 2. [`connect`] — mint the host's certificate, open the upstream
//!    connection, and answer the client.
//! 3. [`crate::intercept`] — terminate the client's TLS session, open a
//!    separate verified one to the origin, and proxy HTTP between them. That
//!    lives outside this module because what is inside an intercepted session
//!    may be HTTP/1.1 or HTTP/2, and ALPN decides which.
//!
//! Either route passes its request through [`ruled`] first, which asks the
//! rule engine what should happen to it and carries out the answer: a
//! request may be blocked here, answered from a file here, or sent to a
//! destination other than the one it named. A `CONNECT` is not evaluated —
//! it carries no path, and the requests inside the session it opens are each
//! evaluated on their own.
//!
//! [`reply`] builds the responses the proxy writes itself, on either route,
//! and [`redact`] is what every log line puts a request target through.
//!
//! One host on the forwarded route never leaves this Mac: [`setup`] is where
//! a device that is already going through the proxy asks the proxy for the
//! certificate it would have to trust. It is answered before the rules are
//! consulted, because it is ProxyLab itself rather than traffic.
//!
//! Everything in this module is HTTP/1.1 framing: the protocol a client
//! speaks to reach the proxy at all, and the protocol an origin gets when it
//! has agreed to no other. HTTP/2 framing is [`crate::http2`], and the two
//! meet again at [`crate::origin`] and [`crate::intercept`].
//!
//! A `CONNECT` is not passed through as opaque bytes: what the client checks
//! is a certificate ProxyLab's development CA signed, and the origin's own
//! certificate is verified by ProxyLab instead.

pub mod connect;
pub mod connect_target;
pub mod connection;
pub mod headers;
pub mod redact;
pub mod reply;
pub mod ruled;
pub mod service;
pub mod setup;
pub mod target;
pub mod upstream;

pub use connection::serve;
