//! `diagnostics`: where the core stands, without anything it captured.

mod support;

use std::sync::Arc;

use proxy_ipc::{
    ComponentStatus, DiagnosticEvent, DiagnosticsControl, DiagnosticsFacts, EndpointStatus, OPEN,
};
use serde_json::Value;
use support::{request, TestServer};

#[derive(Debug)]
struct FixedFacts(DiagnosticsFacts);

impl DiagnosticsControl for FixedFacts {
    fn facts(&self) -> DiagnosticsFacts {
        self.0.clone()
    }
}

fn result(response: &Value) -> &Value {
    assert_eq!(response["ok"], true, "expected success: {response}");

    &response["result"]
}

#[tokio::test]
async fn a_server_answers_without_reading_traffic() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&request("d1", "diagnostics")).await;
    let report = result(&response);

    assert_eq!(report["core"], "running");
    assert_eq!(report["storage"]["state"], "open");
    assert_eq!(report["ipc"]["state"], "serving");
    assert_eq!(report["localhost"]["state"], "unknown");
    assert_eq!(report["transparent"]["state"], "unknown");
    assert!(report.get("lan").is_none(), "{report}");
    assert_eq!(report["recent"], serde_json::json!([]));
    assert!(report["version"].is_string());
}

#[tokio::test]
async fn a_report_drops_a_secret_a_query_and_a_home_path() {
    let facts = DiagnosticsFacts {
        version: "0.1.0".to_owned(),
        localhost: EndpointStatus::listening("127.0.0.1:9090".to_owned()),
        transparent: EndpointStatus::unavailable(
            "could not bind /Users/ada/.proxylab/bridge".to_owned(),
        ),
        storage: ComponentStatus::ok(OPEN),
        recent: vec![DiagnosticEvent {
            level: "error".to_owned(),
            component: "upstream".to_owned(),
            operation: "connect".to_owned(),
            category: "connection".to_owned(),
            message: "failed https://alice:hunter2@example.com/a?token=secret".to_owned(),
            endpoint: Some("https://alice:pw@example.com/a?x=1".to_owned()),
            exchange_id: Some("not an id".to_owned()),
            duration_ms: Some(4),
            at_ms: 9,
        }],
    };
    let server = TestServer::diagnosing(Arc::new(FixedFacts(facts))).await;
    let mut client = server.connect().await;

    let response = client.request(&request("d2", "diagnostics")).await;
    let report = result(&response);
    let json = report.to_string();

    assert!(!json.contains("hunter2"), "{json}");
    assert!(!json.contains("token"), "{json}");
    assert!(!json.contains("secret"), "{json}");
    assert!(!json.contains("ada"), "{json}");
    assert!(!json.contains("alice"), "{json}");
    assert_eq!(report["localhost"]["address"], "127.0.0.1:9090");
    assert_eq!(report["core"], "running");
    assert!(report["recent"][0].get("endpoint").is_none(), "{json}");
    assert!(report["recent"][0].get("exchange_id").is_none(), "{json}");
    assert_eq!(report["recent"][0]["duration_ms"], 4);
}
