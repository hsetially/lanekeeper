//! Constructors for the generated clients and servers.
//!
//! Every client and server is built here so that zstd and the message-size limit are never forgotten (P4).
//! Both ends send zstd and accept it. A server built without `accept_compressed` answers a zstd client with
//! `Unimplemented`, so build servers with these constructors only (the tests pin this down).
//!
//! There is no `connect(dst)` helper on the generated clients (it would clash with the `Connect` RPC):
//! build a `tonic::transport::Channel` yourself (with mTLS, S5) and pass it to [`agent_client`].

use tonic::client::GrpcService;
use tonic::codec::CompressionEncoding;
use tonic::codegen::{Body, Bytes, StdError};

use crate::limits::MAX_MESSAGE_BYTES;
use crate::pb;

const ZSTD: CompressionEncoding = CompressionEncoding::Zstd;

/// The `Agent` service, with zstd and the message limit.
pub fn agent_server<T: pb::agent_server::Agent>(service: T) -> pb::agent_server::AgentServer<T> {
    pb::agent_server::AgentServer::new(service)
        .accept_compressed(ZSTD)
        .send_compressed(ZSTD)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES)
}

/// The `Sentinel` service, with zstd and the message limit.
pub fn sentinel_server<T: pb::sentinel_server::Sentinel>(
    service: T,
) -> pb::sentinel_server::SentinelServer<T> {
    pb::sentinel_server::SentinelServer::new(service)
        .accept_compressed(ZSTD)
        .send_compressed(ZSTD)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES)
}

/// An `Agent` client over `channel`, with zstd and the message limit.
pub fn agent_client<T>(channel: T) -> pb::agent_client::AgentClient<T>
where
    T: GrpcService<tonic::body::Body>,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
{
    pb::agent_client::AgentClient::new(channel)
        .send_compressed(ZSTD)
        .accept_compressed(ZSTD)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES)
}

/// A `Sentinel` client over `channel`, with zstd and the message limit.
pub fn sentinel_client<T>(channel: T) -> pb::sentinel_client::SentinelClient<T>
where
    T: GrpcService<tonic::body::Body>,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
{
    pb::sentinel_client::SentinelClient::new(channel)
        .send_compressed(ZSTD)
        .accept_compressed(ZSTD)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES)
}
