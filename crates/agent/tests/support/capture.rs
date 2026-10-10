//! Watch everything the agent can put out for planted bytes (T11, S17, S10).
//!
//! A test plants files whose content holds a marker that no real config would contain, runs the agent, and then asks this
//! module where the marker went. Four places are searched:
//!
//! - **the stream**: every message the hub received, encoded as the wire carries it (so the bytes of a `Bytes` field are
//!   found whole) and printed with `Debug` (so a marker inside an error, a path or a log-like field is found too);
//! - **the spool**: every file in the spool directory, byte for byte;
//! - **the logs**: everything `tracing` wrote, at every level, from every thread;
//! - **the metrics**: the text exposition.
//!
//! A search that finds nothing proves nothing unless it could have found something, so the module is tested against a
//! planted leak in each place (`planted_leaks_are_found`), and the tests that use it plant a marker in a file that is
//! allowed to be sent and check that this one is seen.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;

use prost::Message;
use proto::convert::FromAgent;

use super::fake_hub::ConnHandle;
use super::log_capture::LogCapture;

/// A byte string that stands for a file's secret content.
#[derive(Debug, Clone)]
pub struct Marker {
    pub name: &'static str,
    pub bytes: Vec<u8>,
}

impl Marker {
    /// Readable text, as in a PEM body or a properties file.
    pub fn text(name: &'static str, text: &str) -> Self {
        Self {
            name,
            bytes: text.as_bytes().to_vec(),
        }
    }

    /// Bytes that are not text, as in a keystore. Long and odd enough not to occur by chance in a protobuf message.
    pub fn binary(name: &'static str, seed: u8) -> Self {
        let mut bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF];
        bytes.extend((0..40_u8).map(|i| i.wrapping_mul(37).wrapping_add(seed)));
        Self { name, bytes }
    }

    fn is_in(&self, haystack: &[u8]) -> bool {
        !self.bytes.is_empty()
            && haystack
                .windows(self.bytes.len())
                .any(|w| w == self.bytes.as_slice())
    }
}

/// Where the markers are searched for.
pub struct Capture {
    markers: Vec<Marker>,
    logs: LogCapture,
}

impl Capture {
    pub fn new(markers: Vec<Marker>) -> Self {
        Self {
            markers,
            logs: LogCapture::new(),
        }
    }

    /// The log sink to install (`install_global`), so that the logs can be searched.
    pub fn logs(&self) -> &LogCapture {
        &self.logs
    }

    fn found_in(&self, haystack: &[u8]) -> Vec<&'static str> {
        self.markers
            .iter()
            .filter(|m| m.is_in(haystack))
            .map(|m| m.name)
            .collect()
    }

    /// The markers one message carries, encoded or printed.
    pub fn in_message(&self, message: &FromAgent) -> Vec<&'static str> {
        let mut found = self.found_in(&message.clone().into_proto().encode_to_vec());
        for name in self.found_in(format!("{message:?}").as_bytes()) {
            if !found.contains(&name) {
                found.push(name);
            }
        }
        found
    }

    /// `"<marker> in message <n>"` for every marker in any message the hub has received on `conn`.
    pub fn on_stream(&self, conn: &ConnHandle) -> Vec<String> {
        conn.received()
            .iter()
            .enumerate()
            .flat_map(|(i, m)| {
                self.in_message(m)
                    .into_iter()
                    .map(move |name| format!("{name} in stream message {i}"))
            })
            .collect()
    }

    /// `"<marker> in <file>"` for every marker in any file of the spool directory.
    pub fn in_spool(&self, dir: &Path) -> Vec<String> {
        let mut found = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = fs::read(&path).unwrap();
                for name in self.found_in(&bytes) {
                    found.push(format!(
                        "{name} in {}",
                        path.file_name().unwrap().to_string_lossy()
                    ));
                }
            }
        }
        found
    }

    /// `"<marker> in the logs"` for every marker anywhere in the captured logs.
    pub fn in_logs(&self) -> Vec<String> {
        self.found_in(self.logs.text().as_bytes())
            .into_iter()
            .map(|name| format!("{name} in the logs"))
            .collect()
    }

    /// `"<marker> in the metrics"` for every marker in the metrics text.
    pub fn in_metrics(&self, text: &str) -> Vec<String> {
        self.found_in(text.as_bytes())
            .into_iter()
            .map(|name| format!("{name} in the metrics"))
            .collect()
    }
}
