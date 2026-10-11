//! Reading a `text/event-stream` body frame by frame, for the SSE tests (in-process responses and real sockets alike).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use bytes::Bytes;
use futures::{Stream, StreamExt};

/// A stream of body chunks, cut into frames at each blank line.
pub struct Frames<S> {
    stream: S,
    buffer: String,
}

impl<S, E> Frames<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: String::new(),
        }
    }

    /// The next complete frame, including its terminating blank line, or `None` when the body ended. A body that ends in
    /// the middle of a frame is a failure of the test.
    pub async fn next(&mut self) -> Option<String> {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let frame: String = self.buffer.drain(..end + 2).collect();
                return Some(frame);
            }
            if let Some(chunk) = self.stream.next().await {
                self.buffer
                    .push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
            } else {
                assert!(
                    self.buffer.is_empty(),
                    "the body ended inside a frame: {:?}",
                    self.buffer
                );
                return None;
            }
        }
    }

    /// The next frame that is not a comment (a heartbeat) or a `retry:` hint.
    pub async fn next_event(&mut self) -> Option<String> {
        loop {
            let frame = self.next().await?;
            if !frame.starts_with(':') && !frame.starts_with("retry:") {
                return Some(frame);
            }
        }
    }
}

/// The fields of one event frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: Option<String>,
}

pub fn parse(frame: &str) -> Parsed {
    let mut parsed = Parsed {
        id: None,
        event: None,
        data: None,
    };
    for line in frame.lines() {
        if let Some(v) = line.strip_prefix("id: ") {
            parsed.id = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("event: ") {
            parsed.event = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("data: ") {
            parsed.data = Some(v.to_owned());
        }
    }
    parsed
}

/// The `type` of the event in a frame's data.
pub fn kind_of(frame: &str) -> String {
    let data = parse(frame).data.expect("an event frame has data");
    let value: serde_json::Value = serde_json::from_str(&data).unwrap();
    value["type"].as_str().unwrap().to_owned()
}
