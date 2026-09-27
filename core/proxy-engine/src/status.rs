//! Identity and readiness reporting for the proxy engine.
//!
//! This module owns the metadata the engine prints when it starts. It is the
//! single source of truth for the engine's name and reported status.

use std::net::SocketAddr;
use std::path::Path;

/// Human-readable name of the engine.
pub const APP_NAME: &str = "ProxyLab Proxy Engine";

/// The startup report: the engine name, the address it is listening on, and
/// where the development CA certificate is.
///
/// The CA path is printed because interception only works for a client that
/// has been told to trust that certificate, and telling it is the developer's
/// job — nothing in ProxyLab changes a system trust setting. The path names
/// the certificate, which is public material; the private key sitting beside
/// it is never named, printed, or logged.
pub fn startup_report(addr: SocketAddr, ca_directory: &Path) -> String {
    format!(
        "{APP_NAME}\nListening on {addr}\nDevelopment CA certificate: {}",
        ca_directory.join(CA_CERTIFICATE_FILE).display()
    )
}

/// The line saying where captured traffic is being kept.
///
/// The path is printed for the same reason the CA path is: it is where a
/// developer's captured traffic — credentials, cookies, bodies and all — now
/// lives on disk, and that is worth knowing without having to read the
/// source.
///
/// A proxy that could not open its database runs anyway, and says why here.
/// Storing traffic is what a debugging proxy does with what it saw; carrying
/// requests is what it is for, and the second does not depend on the first.
pub fn traffic_report(database: Result<&Path, &str>) -> String {
    match database {
        Ok(path) => format!("Traffic database: {}", path.display()),
        Err(reason) => format!("Traffic is not being stored: {reason}"),
    }
}

/// The line saying where the UI connects to the core.
///
/// The socket is the whole of that boundary, so where it is is worth saying
/// out loud — and a run that is not serving one is worth saying out loud
/// too, because a UI that cannot connect looks the same from the outside as
/// a proxy that is not running.
pub fn ipc_report(socket: Result<&Path, &str>) -> String {
    match socket {
        Ok(path) => format!("IPC socket: {}", path.display()),
        Err(reason) => format!("IPC is not being served: {reason}"),
    }
}

/// The line saying where transparently captured flows enter the proxy.
///
/// Said whether or not anything will use it: a Network Extension that is
/// turned on while this line says the bridge is not listening is a Mac whose
/// traffic the provider is passing straight through, and this is where a
/// developer finds out why.
pub fn transparent_report(bridge: Result<SocketAddr, &str>) -> String {
    match bridge {
        Ok(addr) => format!("Transparent capture bridge: {addr}"),
        Err(reason) => format!("Transparent capture is unavailable: {reason}"),
    }
}

/// Name of the CA certificate file within the CA directory.
///
/// Kept in step with `proxy-tls`, which owns the file itself.
const CA_CERTIFICATE_FILE: &str = "ca-cert.pem";

/// The line reporting which rules the proxy starts with.
///
/// A count and nothing else: a rule's name is text a person typed and its
/// matcher names hosts and paths they are working on, and a startup banner is
/// not the place for either.
pub fn rules_report(engine: Option<&rules::RuleEngine>) -> String {
    match engine {
        Some(engine) => {
            let rules = engine.snapshot();

            format!(
                "Rules: {} loaded, {} enabled",
                rules.len(),
                rules.enabled_count()
            )
        }
        None => "Rules: none (there is no database to keep them in)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    fn addr() -> SocketAddr {
        "127.0.0.1:9090".parse().expect("valid socket address")
    }

    fn ca_directory() -> PathBuf {
        PathBuf::from("/home/dev/.proxylab/ca")
    }

    #[test]
    fn app_name_is_the_engine_name() {
        assert_eq!(APP_NAME, "ProxyLab Proxy Engine");
    }

    #[test]
    fn startup_report_has_name_then_listen_address_then_ca() {
        assert_eq!(
            startup_report(addr(), &ca_directory()),
            "ProxyLab Proxy Engine\nListening on 127.0.0.1:9090\n\
             Development CA certificate: /home/dev/.proxylab/ca/ca-cert.pem"
        );
    }

    #[test]
    fn startup_report_is_three_lines_built_from_the_metadata() {
        let report = startup_report(addr(), &ca_directory());
        let lines: Vec<&str> = report.lines().collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], APP_NAME);
        assert_eq!(lines[1], "Listening on 127.0.0.1:9090");
        assert!(lines[2].starts_with("Development CA certificate: "));
    }

    #[test]
    fn startup_report_shows_the_address_it_was_given() {
        let other: SocketAddr = "127.0.0.1:0".parse().expect("valid socket address");

        assert!(startup_report(other, &ca_directory()).contains("Listening on 127.0.0.1:0"));
    }

    #[test]
    fn the_traffic_report_names_the_database() {
        let report = traffic_report(Ok(Path::new("/home/dev/.proxylab/traffic/traffic.db")));

        assert_eq!(
            report,
            "Traffic database: /home/dev/.proxylab/traffic/traffic.db"
        );
    }

    #[test]
    fn the_traffic_report_says_when_nothing_is_being_stored_and_why() {
        let report = traffic_report(Err("the database file could not be opened"));

        assert_eq!(
            report,
            "Traffic is not being stored: the database file could not be opened"
        );
    }

    #[test]
    fn the_ipc_report_names_the_socket() {
        let report = ipc_report(Ok(Path::new("/home/dev/.proxylab/run/proxylab.sock")));

        assert_eq!(report, "IPC socket: /home/dev/.proxylab/run/proxylab.sock");
    }

    #[test]
    fn the_ipc_report_says_when_nothing_is_being_served_and_why() {
        let report = ipc_report(Err("there is no traffic database to serve"));

        assert_eq!(
            report,
            "IPC is not being served: there is no traffic database to serve"
        );
    }

    #[test]
    fn the_transparent_report_names_where_flows_come_in() {
        let report = transparent_report(Ok("127.0.0.1:9091".parse().expect("address")));

        assert_eq!(report, "Transparent capture bridge: 127.0.0.1:9091");
    }

    #[test]
    fn the_transparent_report_says_when_there_is_no_bridge_and_why() {
        let report = transparent_report(Err("see the message above"));

        assert_eq!(
            report,
            "Transparent capture is unavailable: see the message above"
        );
    }

    #[test]
    fn startup_report_names_the_certificate_and_not_the_key() {
        let report = startup_report(addr(), &ca_directory());

        assert!(report.contains("ca-cert.pem"), "{report}");
        assert!(
            !report.contains("ca-key"),
            "the report should never point at the private key: {report}"
        );
    }
}
