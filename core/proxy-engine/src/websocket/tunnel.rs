//! What the two connections become once the handshake has succeeded.
//!
//! ```text
//! client ⇅ WebSocket frames ⇅ ProxyLab ⇅ WebSocket frames ⇅ origin
//! ```
//!
//! Frames are parsed so each message can be captured, and so a text message
//! can be held at a breakpoint. Bytes that are not frames — which is what a
//! number of existing tests send, and what a peer using an extension would
//! send — are copied the rest of the way unchanged, the same way a
//! `CONNECT` tunnel copies bytes that are not HTTP.
//!
//! An unmodified message is written as the bytes it arrived as. An edited
//! text message is the one case that is re-framed.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use proxy_ipc::ThrottleSettings;
use proxy_storage::StreamDirection;
use tokio::io::{split, AsyncRead, AsyncWrite, AsyncWriteExt};

use super::frame::{FrameReader, Opcode, Read};
use crate::capture::stream::StreamTap;
use crate::controls::breakpoint::Breakpoints;
use crate::controls::pause::{self, WebSocketOutcome};
use crate::controls::throttle;
use crate::controls::Exchange;
use crate::shutdown::Shutdown;

/// Joins the two halves of an upgraded connection, once both are handed over.
#[allow(clippy::too_many_arguments)]
pub async fn join(
    client_addr: SocketAddr,
    url: String,
    client: OnUpgrade,
    origin: OnUpgrade,
    tap: StreamTap,
    exchange: Exchange,
    breakpoints: Arc<Breakpoints>,
    throttling: ThrottleSettings,
    shutdown: Shutdown,
) {
    let (client_io, origin_io) = match tokio::try_join!(client, origin) {
        Ok(halves) => halves,
        Err(error) => {
            eprintln!("{client_addr} WebSocket {url} never opened: {error}");
            return;
        }
    };

    println!("{client_addr} WebSocket open {url}");

    let outcome = tokio::select! {
        outcome = copy(
            client_io,
            origin_io,
            tap,
            exchange,
            breakpoints,
            throttling,
            shutdown.clone(),
        ) => outcome,
        () = shutdown.signalled() => {
            println!("{client_addr} WebSocket closed {url} (proxy stopping)");
            return;
        }
    };

    match outcome {
        Ok((from_client, from_origin)) => println!(
            "{client_addr} WebSocket closed {url} ({from_client} bytes up, {from_origin} down)"
        ),
        Err(error) => eprintln!("{client_addr} WebSocket closed {url}: {error}"),
    }
}

async fn copy(
    client: Upgraded,
    origin: Upgraded,
    tap: StreamTap,
    exchange: Exchange,
    breakpoints: Arc<Breakpoints>,
    throttling: ThrottleSettings,
    shutdown: Shutdown,
) -> io::Result<(u64, u64)> {
    let client = TokioIo::new(client);
    let origin = TokioIo::new(origin);
    let (from_client, to_client) = split(client);
    let (from_origin, to_origin) = split(origin);

    let upload = throttling.upload();
    let download = throttling.download();

    tokio::try_join!(
        one_way(
            from_client,
            to_origin,
            StreamDirection::ClientToServer,
            tap.clone(),
            exchange.clone(),
            Arc::clone(&breakpoints),
            upload,
            shutdown.clone(),
        ),
        one_way(
            from_origin,
            to_client,
            StreamDirection::ServerToClient,
            tap,
            exchange,
            breakpoints,
            download,
            shutdown,
        ),
    )
}

#[allow(clippy::too_many_arguments)]
async fn one_way<R, W>(
    reader: R,
    mut writer: W,
    direction: StreamDirection,
    tap: StreamTap,
    exchange: Exchange,
    breakpoints: Arc<Breakpoints>,
    rate: Option<u64>,
    shutdown: Shutdown,
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = FrameReader::new(reader);
    let mut copied = 0u64;

    loop {
        match reader.next().await? {
            Read::Message(message) => {
                let kind = match message.opcode {
                    Opcode::Text => proxy_storage::StreamKind::WebsocketText,
                    Opcode::Binary => proxy_storage::StreamKind::WebsocketBinary,
                    _ => proxy_storage::StreamKind::WebsocketBinary,
                };
                tap.capture_websocket(direction, kind, &message.payload);

                let bytes = if message.opcode == Opcode::Text {
                    match pause::websocket_text(
                        &breakpoints,
                        &exchange,
                        direction,
                        &message.payload,
                        message.raw(),
                        &shutdown,
                    )
                    .await
                    {
                        WebSocketOutcome::Forward(bytes) => bytes,
                        WebSocketOutcome::Drop => continue,
                    }
                } else {
                    message.raw()
                };

                copied += write_bytes(&mut writer, &bytes, rate).await?;
            }
            Read::Control(frame) => {
                copied += write_bytes(&mut writer, &frame.raw, rate).await?;
            }
            Read::Opaque(lead) => {
                copied += write_bytes(&mut writer, &lead, rate).await?;
                let mut inner = reader.into_inner();
                copied += fallback(&mut inner, &mut writer, rate).await?;
                return Ok(copied);
            }
            Read::Eof => {
                writer.shutdown().await?;
                return Ok(copied);
            }
        }
    }
}

async fn write_bytes<W>(writer: &mut W, bytes: &[u8], rate: Option<u64>) -> io::Result<u64>
where
    W: AsyncWrite + Unpin,
{
    if bytes.is_empty() {
        return Ok(0);
    }

    if let Some(rate) = rate {
        throttle::write_paced(writer, bytes, rate).await?;
    } else {
        writer.write_all(bytes).await?;
    }

    Ok(bytes.len() as u64)
}

async fn fallback<R, W>(reader: &mut R, writer: &mut W, rate: Option<u64>) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    match rate {
        Some(rate) => throttle::copy(reader, writer, rate).await,
        None => {
            let copied = tokio::io::copy(reader, writer).await?;
            writer.shutdown().await?;
            Ok(copied)
        }
    }
}
