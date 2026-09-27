//! The socket's life: binding it, sharing it, and cleaning it up.

mod support;

use support::{request, Client, TestServer};

use std::os::unix::fs::{FileTypeExt, PermissionsExt};

use proxy_ipc::{IpcConfig, IpcError, IpcServer};
use tempfile::TempDir;

#[tokio::test]
async fn a_server_creates_the_socket_it_listens_on() {
    let server = TestServer::start().await;

    let metadata = std::fs::symlink_metadata(server.socket()).expect("the socket is there");

    assert!(metadata.file_type().is_socket(), "not a socket");

    server.stop().await;
}

#[tokio::test]
async fn the_socket_is_restricted_to_the_user_that_made_it() {
    let server = TestServer::start().await;

    let socket = std::fs::metadata(server.socket()).expect("the socket is there");
    let directory = std::fs::metadata(
        server
            .socket()
            .parent()
            .expect("the socket has a directory"),
    )
    .expect("the directory is there");

    // Nobody else may connect, and nobody else may even look in the
    // directory the socket is in.
    assert_eq!(socket.permissions().mode() & 0o777, 0o600);
    assert_eq!(directory.permissions().mode() & 0o777, 0o700);

    server.stop().await;
}

#[tokio::test]
async fn a_client_can_connect_and_be_answered() {
    let server = TestServer::start().await;

    let mut client = Client::connect(server.socket()).await;
    let response = client.request(&request("r1", "list_traffic")).await;

    assert_eq!(response["ok"], true);

    server.stop().await;
}

#[tokio::test]
async fn a_client_disconnecting_does_not_stop_the_server() {
    let server = TestServer::start().await;

    let mut first = server.connect().await;
    assert_eq!(
        first.request(&request("r1", "clear_traffic")).await["ok"],
        true
    );
    first.disconnect().await;

    // The whole sequence again on a new connection: the server is still
    // there, still listening, and still answering.
    let mut second = server.connect().await;
    assert_eq!(
        second.request(&request("r2", "clear_traffic")).await["ok"],
        true
    );

    server.stop().await;
}

#[tokio::test]
async fn a_client_that_vanishes_mid_request_does_not_stop_the_server() {
    let server = TestServer::start().await;

    // Half a request, and then gone.
    let mut abrupt = server.connect().await;
    abrupt.send_raw(r#"{"version":1,"request_id":"#).await;
    drop(abrupt);

    let mut after = server.connect().await;
    assert_eq!(
        after.request(&request("r1", "clear_traffic")).await["ok"],
        true
    );

    server.stop().await;
}

#[tokio::test]
async fn many_clients_are_served_at_once() {
    let server = TestServer::start().await;

    // Every client connects before any of them is answered, so all eight
    // connections are live at the same time.
    let mut clients = Vec::new();
    for _ in 0..8 {
        clients.push(server.connect().await);
    }

    for (index, client) in clients.iter_mut().enumerate() {
        client
            .send(&request(&format!("r{index}"), "list_traffic"))
            .await;
    }
    for (index, client) in clients.iter_mut().enumerate() {
        let response = client.next_message().await;

        assert_eq!(response["request_id"], format!("r{index}"));
        assert_eq!(response["ok"], true);
    }

    server.stop().await;
}

#[tokio::test]
async fn shutting_down_removes_the_socket() {
    let server = TestServer::start().await;
    let socket = server.socket().to_path_buf();
    assert!(socket.exists());

    server.stop().await;

    assert!(!socket.exists(), "the socket outlived the server");
}

#[tokio::test]
async fn shutting_down_ends_the_clients_that_are_still_connected() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    assert_eq!(
        client.request(&request("r1", "list_traffic")).await["ok"],
        true
    );

    // The server stops while a client is connected and waiting: it must not
    // wait for that client to say anything, and the client must be let go.
    server.stop().await;

    assert!(client.is_closed().await, "the connection was left open");
}

#[tokio::test]
async fn a_socket_left_behind_by_a_dead_process_is_taken_over() {
    let directory = TempDir::new().expect("a temporary directory");
    let socket = directory.path().join("proxylab.sock");
    // What a crash leaves: the file, with nothing listening on it.
    let abandoned = std::os::unix::net::UnixListener::bind(&socket).expect("binds");
    drop(abandoned);
    assert!(socket.exists());

    let server = IpcServer::bind(&IpcConfig::at(&socket)).expect("the stale socket is taken over");

    assert_eq!(server.socket_path(), socket);
    drop(server);
}

#[tokio::test]
async fn a_socket_another_server_is_listening_on_is_left_alone() {
    let server = TestServer::start().await;

    let error = IpcServer::bind(&IpcConfig::at(server.socket()))
        .expect_err("something is already listening");

    assert!(
        matches!(error, IpcError::AlreadyRunning { .. }),
        "{error:?}"
    );
    // Refused rather than taken over: the running server's clients still
    // reach the running server.
    let mut client = server.connect().await;
    assert_eq!(
        client.request(&request("r1", "list_traffic")).await["ok"],
        true
    );

    server.stop().await;
}

#[tokio::test]
async fn a_path_holding_something_that_is_not_a_socket_is_refused() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = directory.path().join("proxylab.sock");
    std::fs::write(&path, b"someone else's file").expect("a file in the way");

    let error = IpcServer::bind(&IpcConfig::at(&path)).expect_err("that path is taken");

    assert!(matches!(error, IpcError::PathOccupied { .. }), "{error:?}");
    assert!(path.is_file(), "another file was removed");
}

#[tokio::test]
async fn a_socket_path_the_operating_system_will_not_take_is_refused_with_a_reason() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = directory.path().join("x".repeat(120));

    let error = IpcServer::bind(&IpcConfig::at(&path)).expect_err("that path is too long");

    assert!(
        matches!(error, IpcError::SocketPathTooLong { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("at most"), "{error}");
}

#[tokio::test]
async fn a_server_that_is_dropped_without_running_still_cleans_up() {
    let directory = TempDir::new().expect("a temporary directory");
    let socket = directory.path().join("proxylab.sock");

    let server = IpcServer::bind(&IpcConfig::at(&socket)).expect("binds");
    assert!(socket.exists());
    drop(server);

    assert!(!socket.exists(), "the socket was left behind");
}

#[tokio::test]
async fn a_socket_can_be_bound_again_once_its_server_has_stopped() {
    let directory = TempDir::new().expect("a temporary directory");
    let socket = directory.path().join("proxylab.sock");

    for _ in 0..3 {
        let server = IpcServer::bind(&IpcConfig::at(&socket)).expect("binds");
        assert_eq!(server.socket_path(), socket);
        drop(server);
    }
}
