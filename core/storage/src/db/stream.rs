//! Persisting stream messages: WebSocket frames and SSE events.
//!
//! One row per captured message, hanging off the exchange it belongs to.
//! Payloads are placed the way a captured HTTP body is — inline or in a
//! file — so a megabyte of JSON on a WebSocket does not sit in a row a list
//! query scans past.

use rusqlite::types::Value;
use rusqlite::{params_from_iter, OptionalExtension, Row};

use crate::body::BodyCapture;
use crate::stream::{
    StreamDirection, StreamKind, StreamMessage, StreamMessageId, StreamMessagePage,
    StreamMessageSummary,
};
use crate::traffic::ExchangeId;

use super::bodies::{self, Placement};
use super::encoding::{self, timestamp_columns, timestamp_from_columns};
use super::error::StorageError;
use super::query::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use super::store::Storage;

const INSERT: &str = "\
INSERT INTO stream_messages (
    id, exchange_id, sequence, started_at_ms, started_at_subms_nanos,
    direction, kind, event_name, event_id, retry_ms,
    payload_size, payload_captured_len, payload_truncated, payload_inline, payload_file
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";

const SELECT_SUMMARY: &str = "\
SELECT id, exchange_id, sequence, started_at_ms, started_at_subms_nanos, \
       direction, kind, event_name, event_id, retry_ms, \
       payload_size, payload_captured_len, payload_truncated \
FROM stream_messages";

const SELECT_ONE: &str = "\
SELECT id, exchange_id, sequence, started_at_ms, started_at_subms_nanos, \
       direction, kind, event_name, event_id, retry_ms, \
       payload_size, payload_captured_len, payload_truncated, payload_inline, payload_file \
FROM stream_messages WHERE id = ?1";

impl Storage {
    /// Stores one captured stream message.
    ///
    /// Fails if the parent exchange is not stored yet — the foreign key is
    /// what keeps a message from outliving the handshake it belongs to.
    pub fn insert_stream_message(&self, message: &StreamMessage) -> Result<(), StorageError> {
        let staged = bodies::stage_stream(self.config(), message.id, &message.payload)?;

        let stored = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                INSERT,
                params_from_iter(message_params(message, staged.placement())?),
            )?;
            staged.promote()?;
            transaction.commit()?;
            Ok(())
        });

        if stored.is_err() {
            staged.abandon();
        }

        stored
    }

    /// A page of stream messages for `exchange`, oldest first.
    ///
    /// `after` is the last sequence number already held; the next page is
    /// everything with a greater sequence. Payloads are not loaded: a list
    /// is a list, and a client that wants one payload asks for that message.
    pub fn list_stream_messages(
        &self,
        exchange: ExchangeId,
        after: Option<u64>,
        limit: Option<usize>,
    ) -> Result<StreamMessagePage, StorageError> {
        let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
        let fetch = limit + 1;

        let messages = self.with_connection(|connection| {
            let sql = if after.is_some() {
                format!(
                    "{SELECT_SUMMARY} WHERE exchange_id = ?1 AND sequence > ?2 \
                     ORDER BY sequence ASC LIMIT ?3"
                )
            } else {
                format!(
                    "{SELECT_SUMMARY} WHERE exchange_id = ?1 \
                     ORDER BY sequence ASC LIMIT ?2"
                )
            };

            let mut statement = connection.prepare(&sql)?;
            let rows = if let Some(after) = after {
                statement.query_map(
                    rusqlite::params![exchange.as_text(), after as i64, fetch as i64],
                    read_summary,
                )?
            } else {
                statement.query_map(
                    rusqlite::params![exchange.as_text(), fetch as i64],
                    read_summary,
                )?
            };

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(StorageError::from)
        })?;

        let mut messages = messages.into_iter().collect::<Result<Vec<_>, _>>()?;

        let next_after = if messages.len() > limit {
            messages.pop();
            messages.last().map(|message| message.sequence)
        } else {
            None
        };

        Ok(StreamMessagePage {
            messages,
            next_after,
        })
    }

    /// One stream message, whole, payload included.
    pub fn get_stream_message(
        &self,
        id: StreamMessageId,
    ) -> Result<Option<StreamMessage>, StorageError> {
        let stored = self.with_connection(|connection| {
            match connection
                .query_row(SELECT_ONE, [id.as_text()], read_message)
                .optional()?
            {
                None => Ok(None),
                Some(row) => row.map(Some),
            }
        })?;

        let Some((mut message, file)) = stored else {
            return Ok(None);
        };

        if let Some(name) = file {
            let data = bodies::read(self.config(), &name)?;
            message.payload = encoding::body_from_columns(
                data,
                i64::try_from(message.payload.size()).unwrap_or(i64::MAX),
                message.payload.is_truncated(),
            );
        }

        Ok(Some(message))
    }
}

pub(super) fn stream_files(
    connection: &rusqlite::Connection,
    exchange: ExchangeId,
) -> Result<Vec<String>, StorageError> {
    let mut statement =
        connection.prepare("SELECT payload_file FROM stream_messages WHERE exchange_id = ?1")?;
    let files = statement
        .query_map([exchange.as_text()], |row| row.get::<_, Option<String>>(0))?
        .filter_map(|file| match file {
            Ok(Some(name)) => Some(Ok(name)),
            Ok(None) => None,
            Err(error) => Some(Err(error.into())),
        })
        .collect::<Result<Vec<_>, StorageError>>()?;

    Ok(files)
}

fn message_params(
    message: &StreamMessage,
    placement: &Placement,
) -> Result<Vec<Value>, StorageError> {
    let (started_at_ms, started_at_subms_nanos) = timestamp_columns(message.captured_at);

    Ok(vec![
        Value::Text(message.id.as_text()),
        Value::Text(message.exchange_id.as_text()),
        Value::Integer(i64::try_from(message.sequence).unwrap_or(i64::MAX)),
        Value::Integer(started_at_ms),
        Value::Integer(started_at_subms_nanos),
        Value::Text(message.direction.as_str().to_owned()),
        Value::Text(message.kind.as_str().to_owned()),
        optional_text(message.event_name.as_deref()),
        optional_text(message.event_id.as_deref()),
        optional_integer(message.retry_ms),
        Value::Integer(i64::try_from(message.payload.size()).unwrap_or(i64::MAX)),
        Value::Integer(i64::try_from(message.payload.captured_len()).unwrap_or(i64::MAX)),
        Value::Integer(i64::from(message.payload.is_truncated())),
        match placement.inline() {
            Some(data) => Value::Blob(data.to_vec()),
            None => Value::Null,
        },
        match placement.file() {
            Some(name) => Value::Text(name.to_owned()),
            None => Value::Null,
        },
    ])
}

fn optional_text(value: Option<&str>) -> Value {
    match value {
        Some(text) => Value::Text(text.to_owned()),
        None => Value::Null,
    }
}

fn optional_integer(value: Option<u64>) -> Value {
    match value {
        Some(number) => Value::Integer(i64::try_from(number).unwrap_or(i64::MAX)),
        None => Value::Null,
    }
}

fn read_summary(row: &Row<'_>) -> rusqlite::Result<Result<StreamMessageSummary, StorageError>> {
    Ok(summary_from_row(row))
}

fn read_message(
    row: &Row<'_>,
) -> rusqlite::Result<Result<(StreamMessage, Option<String>), StorageError>> {
    Ok((|| {
        let summary = summary_from_row(row)?;
        let inline: Option<Vec<u8>> = row.get(13)?;
        let file: Option<String> = row.get(14)?;
        let payload = match (inline, file.as_deref()) {
            (Some(data), None) => encoding::body_from_columns(
                data,
                i64::try_from(summary.payload_size).unwrap_or(i64::MAX),
                summary.payload_truncated,
            ),
            (None, Some(_)) => BodyCapture::truncated(Vec::new(), summary.payload_size),
            (None, None) => {
                if summary.payload_size == 0 {
                    BodyCapture::empty()
                } else {
                    encoding::body_from_columns(
                        Vec::new(),
                        i64::try_from(summary.payload_size).unwrap_or(i64::MAX),
                        summary.payload_truncated,
                    )
                }
            }
            (Some(_), Some(_)) => {
                return Err(StorageError::unreadable("stream message payload"));
            }
        };

        Ok((
            StreamMessage {
                id: summary.id,
                exchange_id: summary.exchange_id,
                sequence: summary.sequence,
                captured_at: summary.captured_at,
                direction: summary.direction,
                kind: summary.kind,
                event_name: summary.event_name,
                event_id: summary.event_id,
                retry_ms: summary.retry_ms,
                payload,
            },
            file,
        ))
    })())
}

fn summary_from_row(row: &Row<'_>) -> Result<StreamMessageSummary, StorageError> {
    let id: String = row.get(0)?;
    let exchange: String = row.get(1)?;
    let sequence: i64 = row.get(2)?;
    let started_at_ms: i64 = row.get(3)?;
    let started_at_subms_nanos: i64 = row.get(4)?;
    let direction: String = row.get(5)?;
    let kind: String = row.get(6)?;
    let event_name: Option<String> = row.get(7)?;
    let event_id: Option<String> = row.get(8)?;
    let retry_ms: Option<i64> = row.get(9)?;
    let payload_size: i64 = row.get(10)?;
    let payload_captured_len: i64 = row.get(11)?;
    let payload_truncated: i64 = row.get(12)?;

    Ok(StreamMessageSummary {
        id: id
            .parse()
            .map_err(|_| StorageError::unreadable("stream message id"))?,
        exchange_id: exchange
            .parse()
            .map_err(|_| StorageError::unreadable("exchange id"))?,
        sequence: sequence.max(0) as u64,
        captured_at: timestamp_from_columns(started_at_ms, started_at_subms_nanos),
        direction: match direction.as_str() {
            "client_to_server" => StreamDirection::ClientToServer,
            "server_to_client" => StreamDirection::ServerToClient,
            _ => return Err(StorageError::unreadable("stream direction")),
        },
        kind: match kind.as_str() {
            "websocket_text" => StreamKind::WebsocketText,
            "websocket_binary" => StreamKind::WebsocketBinary,
            "sse" => StreamKind::Sse,
            _ => return Err(StorageError::unreadable("stream kind")),
        },
        event_name,
        event_id,
        retry_ms: retry_ms.map(|value| value.max(0) as u64),
        payload_size: payload_size.max(0) as u64,
        payload_captured_len: payload_captured_len.max(0) as u64,
        payload_truncated: payload_truncated != 0,
    })
}
