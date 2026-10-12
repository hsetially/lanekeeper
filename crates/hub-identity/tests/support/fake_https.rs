//! An `HttpsClient` that answers from a closure and records what it was asked.

use std::sync::Mutex;

use async_trait::async_trait;
use hub_identity::http::{HttpsClient, HttpsError, HttpsRequest, HttpsResponse};

/// What the fake saw. The body of a `POST` is kept as text so tests can look at the form.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub url: String,
    pub body: String,
}

type Handler = Box<dyn Fn(&HttpsRequest) -> Result<HttpsResponse, HttpsError> + Send + Sync>;

pub struct FakeHttps {
    handler: Handler,
    seen: Mutex<Vec<Recorded>>,
}

impl FakeHttps {
    pub fn new(
        handler: impl Fn(&HttpsRequest) -> Result<HttpsResponse, HttpsError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Box::new(handler),
            seen: Mutex::new(Vec::new()),
        }
    }

    pub fn seen(&self) -> Vec<Recorded> {
        self.seen.lock().unwrap().clone()
    }

    pub fn count(&self, method: &str, url_part: &str) -> usize {
        self.seen()
            .iter()
            .filter(|r| r.method == method && r.url.contains(url_part))
            .count()
    }
}

#[async_trait]
impl HttpsClient for FakeHttps {
    async fn send(&self, request: HttpsRequest) -> Result<HttpsResponse, HttpsError> {
        self.seen.lock().unwrap().push(Recorded {
            method: request.method().to_string(),
            url: request.url().to_string(),
            body: String::from_utf8_lossy(request.body()).into_owned(),
        });
        (self.handler)(&request)
    }
}

pub fn json_response(status: u16, body: &str, cache_control: Option<&str>) -> HttpsResponse {
    let mut headers = http::HeaderMap::new();
    if let Some(cc) = cache_control {
        headers.insert(http::header::CACHE_CONTROL, cc.parse().unwrap());
    }
    HttpsResponse {
        status,
        headers,
        body: bytes::Bytes::from(body.to_owned()),
    }
}
