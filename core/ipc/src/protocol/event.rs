//! What the server tells a subscribed client without being asked.
//!
//! ```text
//! {"version":1,"type":"traffic_added","exchange":{…summary…}}
//! {"version":1,"type":"traffic_lost","missed":12}
//! {"version":1,"type":"breakpoint_paused","paused":{…}}
//! {"version":1,"type":"breakpoint_released","pause_id":"…","reason":"continued"}
//! ```
//!
//! An event carries no `request_id` and no `ok`: it answers nothing. A client
//! tells the two apart by `ok`, which every response has and no event does.
//!
//! # Two streams, two subscriptions
//!
//! A client receives traffic events after `subscribe` and paused exchanges
//! after `watch_breakpoints`, and the two are separate on purpose: watching
//! breakpoints is what makes the proxy pause at all, and a client that only
//! wants to see traffic go past must not cause any.
//!
//! # An event is a summary, not an exchange
//!
//! [`EventBody::TrafficAdded`] carries the same row the traffic list shows —
//! method, URL, host, status, timing — and none of the headers or captured
//! bodies. That is the whole point: an event is sent to every subscriber for
//! every request that goes through the proxy, and putting a megabyte of
//! captured body in one would make a busy proxy expensive to watch. A client
//! that wants the rest asks for that one exchange by id.
//!
//! # Falling behind is an event too
//!
//! A subscriber that cannot keep up misses exchanges — the event stream is
//! bounded, and drops rather than making the proxy wait. Rather than leave
//! the client with a list that is quietly wrong, it is told how many it
//! missed, which is its cue to ask for the list again.

use serde::{Deserialize, Serialize};

use proxy_storage::{ExchangeSummary, StreamMessageSummary};

use crate::breakpoint::{PauseId, PausedExchange, ReleaseReason};

use super::PROTOCOL_VERSION;

/// Something that happened, sent to a subscribed client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// The protocol version this server speaks.
    pub version: u32,
    /// What happened.
    #[serde(flatten)]
    pub body: EventBody,
}

impl Event {
    /// An exchange the proxy has just finished.
    pub fn traffic_added(exchange: ExchangeSummary) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body: EventBody::TrafficAdded { exchange },
        }
    }

    /// Exchanges this subscriber was too slow to receive.
    pub fn traffic_lost(missed: u64) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body: EventBody::TrafficLost { missed },
        }
    }

    /// An exchange a breakpoint is holding still.
    pub fn breakpoint_paused(paused: PausedExchange) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body: EventBody::BreakpointPaused {
                paused: Box::new(paused),
            },
        }
    }

    /// An exchange that is not being held any more.
    pub fn breakpoint_released(pause_id: PauseId, reason: ReleaseReason) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body: EventBody::BreakpointReleased { pause_id, reason },
        }
    }

    /// A WebSocket message or SSE event the proxy has just captured.
    pub fn stream_message_added(message: StreamMessageSummary) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body: EventBody::StreamMessageAdded { message },
        }
    }

    /// Stream-message summaries this subscriber was too slow to receive.
    pub fn stream_messages_lost(missed: u64) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body: EventBody::StreamMessagesLost { missed },
        }
    }
}

/// What an event is about.
///
/// One variant is much larger than the other, which is what it is: an event
/// is built, serialized, and dropped, and never held in a collection where
/// the difference would add up. Boxing the summary would add an allocation
/// per event and put a `Box` in the middle of a type clients match on, for
/// nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventBody {
    /// The proxy finished an exchange.
    TrafficAdded {
        /// The row a traffic list would show for it.
        exchange: ExchangeSummary,
    },
    /// This subscriber fell behind and exchanges were dropped for it.
    TrafficLost {
        /// How many it did not receive.
        missed: u64,
    },
    /// A breakpoint is holding an exchange still, waiting to be told what to
    /// do with it.
    ///
    /// Boxed, unlike the summary above: this one carries a header section and
    /// as much of a body as the boundary will pass, which is far larger than
    /// everything else an event may be. It is also the one variant that is
    /// occasionally held rather than sent and dropped, because a client keeps
    /// it for as long as it is showing the pause.
    BreakpointPaused {
        /// The exchange, and everything about it that may be edited.
        paused: Box<PausedExchange>,
    },
    /// An exchange that was being held is not any more.
    ///
    /// Sent for every ending, including one this client asked for: a client
    /// showing the pause has to stop showing it, and a second client watching
    /// the same proxy is told the same thing.
    BreakpointReleased {
        /// Which pause ended.
        pause_id: PauseId,
        /// Why it did.
        reason: ReleaseReason,
    },
    /// A stream message was captured on an exchange that is already listed.
    ///
    /// A summary, not the payload: the same split the traffic list already
    /// has. A client that wants the bytes asks for that one message.
    StreamMessageAdded {
        /// The row a message timeline would show for it.
        message: StreamMessageSummary,
    },
    /// This subscriber fell behind and stream-message summaries were dropped.
    StreamMessagesLost {
        /// How many it did not receive.
        missed: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::SocketAddr;

    use proxy_storage::{
        BodyCapture, CapturedRequest, CapturedResponse, ConnectionInfo, Destination, ExchangeId,
        HeaderValue, Headers, HttpVersion, TrafficExchange,
    };

    use crate::breakpoint::{BreakpointDirection, BreakpointId, PausedRequest};

    /// A request being held still, carrying everything a client may edit.
    fn paused(pause_id: PauseId) -> PausedExchange {
        let mut headers = Headers::new();
        headers.push("Accept", HeaderValue::Text("application/json".into()));

        PausedExchange {
            pause_id,
            breakpoint_id: BreakpointId::next(),
            direction: BreakpointDirection::Request,
            method: "POST".to_owned(),
            url: "https://api.example.com/orders".to_owned(),
            request: Some(PausedRequest {
                method: "POST".to_owned(),
                url: "https://api.example.com/orders".to_owned(),
                headers,
                body: BodyCapture::complete(b"{}".to_vec()),
            }),
            response: None,
            websocket: None,
        }
    }

    /// An exchange carrying everything an event must not repeat.
    fn exchange() -> TrafficExchange {
        let client: SocketAddr = "127.0.0.1:54321".parse().expect("a valid address");
        let mut exchange = TrafficExchange::new(
            ExchangeId::next(),
            ConnectionInfo::http(client, Some(Destination::new("example.com", 80))),
            CapturedRequest::new("POST", "http://example.com/orders", HttpVersion::Http11),
        );
        exchange.request.headers.push(
            "Authorization",
            HeaderValue::Text("Bearer sk-secret".into()),
        );
        exchange.request.body = BodyCapture::complete(b"password=hunter2".to_vec());

        let mut response = CapturedResponse::new(201, HttpVersion::Http11);
        response
            .headers
            .push("Set-Cookie", HeaderValue::Text("session=abc".into()));
        response.body = BodyCapture::complete(vec![b'x'; 4_096]);
        exchange.response = Some(response);

        exchange
    }

    #[test]
    fn an_added_exchange_is_named_and_summarised() {
        let exchange = exchange();
        let event = Event::traffic_added(ExchangeSummary::of(&exchange));

        let json = serde_json::to_value(&event).expect("serializes");

        assert_eq!(json["version"], 1);
        assert_eq!(json["type"], "traffic_added");
        assert_eq!(json["exchange"]["id"], exchange.id.to_string());
        assert_eq!(json["exchange"]["method"], "POST");
        assert_eq!(json["exchange"]["url"], "http://example.com/orders");
        assert_eq!(json["exchange"]["status"], 201);
    }

    #[test]
    fn an_event_carries_no_headers_and_no_bodies() {
        let event = Event::traffic_added(ExchangeSummary::of(&exchange()));

        let line = serde_json::to_string(&event).expect("serializes");

        for secret in ["sk-secret", "hunter2", "session=abc", "Authorization"] {
            assert!(!line.contains(secret), "{line} carries {secret}");
        }
        assert!(!line.contains("body"), "{line}");
        // A summary of one exchange is a small message, whatever the
        // exchange was carrying.
        assert!(line.len() < 1_024, "an event of {} bytes", line.len());
    }

    #[test]
    fn an_event_is_told_apart_from_a_response_by_having_no_ok() {
        let json = serde_json::to_value(Event::traffic_lost(3)).expect("serializes");

        assert!(json.get("ok").is_none(), "{json}");
        assert!(json.get("request_id").is_none(), "{json}");
    }

    #[test]
    fn a_subscriber_that_fell_behind_is_told_how_much_it_missed() {
        let json = serde_json::to_value(Event::traffic_lost(12)).expect("serializes");

        assert_eq!(json["type"], "traffic_lost");
        assert_eq!(json["missed"], 12);
    }

    #[test]
    fn a_paused_exchange_names_the_pause_and_what_may_be_edited() {
        let pause = PauseId::next();
        let event = Event::breakpoint_paused(paused(pause));
        let json = serde_json::to_value(&event).expect("serializes");

        assert_eq!(json["type"], "breakpoint_paused");
        assert_eq!(json["paused"]["pause_id"], pause.to_string());
        assert_eq!(json["paused"]["direction"], "request");
        assert_eq!(json["paused"]["request"]["method"], "POST");
        assert!(json.get("ok").is_none(), "{json}");
    }

    #[test]
    fn a_released_pause_says_why_it_ended() {
        let pause = PauseId::next();
        let json =
            serde_json::to_value(Event::breakpoint_released(pause, ReleaseReason::Abandoned))
                .expect("serializes");

        assert_eq!(json["type"], "breakpoint_released");
        assert_eq!(json["pause_id"], pause.to_string());
        assert_eq!(json["reason"], "abandoned");
    }

    #[test]
    fn a_stream_message_event_is_a_summary_without_a_payload() {
        let summary = proxy_storage::StreamMessage {
            id: proxy_storage::StreamMessageId::next(),
            exchange_id: proxy_storage::ExchangeId::next(),
            sequence: 0,
            captured_at: proxy_storage::Timestamp::now(),
            direction: proxy_storage::StreamDirection::ClientToServer,
            kind: proxy_storage::StreamKind::WebsocketText,
            event_name: None,
            event_id: None,
            retry_ms: None,
            payload: proxy_storage::BodyCapture::complete(b"secret".to_vec()),
        }
        .summary();
        let event = Event::stream_message_added(summary.clone());
        let json = serde_json::to_value(&event).expect("serializes");
        let line = serde_json::to_string(&event).expect("serializes");

        assert_eq!(json["type"], "stream_message_added");
        assert_eq!(json["message"]["id"], summary.id.to_string());
        assert!(json["message"].get("payload").is_none(), "{json}");
        assert!(!line.contains("secret"), "{line}");
    }

    #[test]
    fn an_event_round_trips_through_json() {
        for event in [
            Event::traffic_added(ExchangeSummary::of(&exchange())),
            Event::traffic_lost(7),
            Event::breakpoint_paused(paused(PauseId::next())),
            Event::breakpoint_released(PauseId::next(), ReleaseReason::Continued),
            Event::stream_messages_lost(4),
        ] {
            let line = serde_json::to_string(&event).expect("serializes");
            let back: Event = serde_json::from_str(&line).expect("deserializes");

            assert_eq!(back, event);
        }
    }
}
