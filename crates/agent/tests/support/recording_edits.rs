//! A [`TreeEdits`] that remembers what it was told, so a test can see which tree edits a file operation asked for.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use agent::fileops::TreeEdits;
use agent::tree::FileLeaf;
use async_trait::async_trait;
use domain::NfsPath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Written(String, FileLeaf),
    Removed(String),
}

#[derive(Debug, Default)]
pub struct RecordingEdits {
    edits: Mutex<Vec<Edit>>,
}

impl RecordingEdits {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn edits(&self) -> Vec<Edit> {
        self.edits.lock().unwrap().clone()
    }
}

#[async_trait]
impl TreeEdits for RecordingEdits {
    async fn written(&self, path: &NfsPath, leaf: FileLeaf) {
        self.edits
            .lock()
            .unwrap()
            .push(Edit::Written(path.as_str().to_owned(), leaf));
    }

    async fn removed(&self, path: &NfsPath) {
        self.edits
            .lock()
            .unwrap()
            .push(Edit::Removed(path.as_str().to_owned()));
    }
}
