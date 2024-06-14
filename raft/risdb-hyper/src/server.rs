use super::helper::error;
use crate::structs::{
    get_response, put_response, GetFailure, GetRequest, GetResponse, GetSuccess, PutFailure,
    PutRequest, PutResponse, PutSuccess,
};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Body, Bytes, Incoming};
use hyper::service::Service;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use prost::{DecodeError, Message};
use raft_grpc::database::RisDb;
use raft_grpc::structs::{
    GetValueInput, GetValueOutput, ProposeValueInput, ProposeValueOutput, RaftMessage,
};
use raft_grpc::{shared, ReadAndWrite};
use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::{fs, io};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceBuilder;
use tracing::info;

pub async fn run(
    addr: SocketAddr,
    certs_path: &PathBuf,
    key_path: &PathBuf,
    database: RisDb,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let certs = load_certs(certs_path)?;
    let key = load_private_key(key_path)?;

    let listener = TcpListener::bind(addr).await?;

    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| error(e.to_string()))?;
    server_config.alpn_protocols = vec![b"h2".to_vec()];
    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));

    let svc = RisDbSvc {
        risdb: Arc::new(database),
    };

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = listener.accept().await?;

        // Spawn a tokio task to serve multiple connections concurrently
        let tls_acceptor = tls_acceptor.clone();
        let svc = svc.clone();
        tokio::spawn(async move {
            let tls_stream = match tls_acceptor.accept(stream).await {
                Ok(tls_stream) => tls_stream,
                Err(err) => {
                    eprintln!("failed to perform tls handshake: {err:#}");
                    return;
                }
            };

            // N.B. should use hyper service_fn here, since it's required to be implemented hyper Service trait!
            let svc = ServiceBuilder::new().layer_fn(Logger::new).service(svc);
            if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(tls_stream), svc)
                .await
            {
                eprintln!("failed to serve connection: {err:#}");
            }
        });
    }
}

// Load public certificate from file.
fn load_certs(filename: &PathBuf) -> io::Result<Vec<CertificateDer<'static>>> {
    // Open certificate file.
    let certfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {:?}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(certfile);

    // Load and return certificate.
    rustls_pemfile::certs(&mut reader).collect()
}

// Load private key from file.
fn load_private_key(filename: &PathBuf) -> io::Result<PrivateKeyDer<'static>> {
    // Open keyfile.
    let keyfile = fs::File::open(filename)
        .map_err(|e| error(format!("failed to open {:?}: {}", filename, e)))?;
    let mut reader = io::BufReader::new(keyfile);

    // Load and return a single private key.
    rustls_pemfile::private_key(&mut reader).map(|key| key.unwrap())
}

#[derive(Debug, Clone)]
struct RisDbSvc {
    risdb: Arc<RisDb>,
}

impl Service<Request<Incoming>> for RisDbSvc {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let cloned = self.clone();
        Box::pin(async move { cloned.serve_request(req).await })
    }
}

impl RisDbSvc {
    async fn serve_request(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
        match (req.method(), req.uri().path()) {
            (&Method::POST, "/get") => self.respond::<GetRequest>(req).await,
            (&Method::POST, "/put") => self.respond::<PutRequest>(req).await,
            // Return 404 Not Found for other routes.
            _ => {
                let mut not_found = Response::new(empty());
                *not_found.status_mut() = StatusCode::NOT_FOUND;
                Ok(not_found)
            }
        }
    }
}

trait Handle<T> {
    async fn handle(&self, input: T) -> Result<BoxBody<Bytes, hyper::Error>, hyper::Error>;
}

/// Needed to convert this crates Value implementation to the Raft
/// crates value implementation
/// TODO: Make it so this isn't necessary either via a shared protobuf definition
///  or by making the API take something that's protobuf agnostic
impl From<crate::structs::Value> for shared::Value {
    fn from(this_crate_value: crate::structs::Value) -> Self {
        Self {
            key: this_crate_value.key,
            value: this_crate_value.value,
        }
    }
}

/// Needed to convert the Raft crates Value implementation to this crates
/// Value implementation.
/// TODO: Make it so this isn't necessary either via a shared protobuf definition
///  or by making the API take something that's protobuf agnostic
impl From<shared::Value> for crate::structs::Value {
    fn from(raft_crate_value: shared::Value) -> Self {
        Self {
            key: raft_crate_value.key,
            value: raft_crate_value.value,
        }
    }
}

impl Handle<GetRequest> for RisDbSvc {
    async fn handle(
        &self,
        input: GetRequest,
    ) -> Result<BoxBody<Bytes, hyper::Error>, hyper::Error> {
        let get_value_output = self
            .risdb
            .database
            .get_value(GetValueInput {
                request_id: input.request_id.to_owned(),
                keys: input
                    .keys
                    .into_iter()
                    .map(|this_crate_value| this_crate_value.into())
                    .collect(),
            })
            .await;

        let response = match get_value_output {
            Ok(output) => GetResponse {
                request_id: input.request_id,
                response: Some(get_response::Response::Success(GetSuccess {
                    values: output
                        .values
                        .into_iter()
                        .map(|raft_crate_value| raft_crate_value.into())
                        .collect(),
                })),
            },
            Err(read_error) => GetResponse {
                request_id: input.request_id,
                response: Some(get_response::Response::Failure(GetFailure {
                    reason: format!("Failed to get: {:?}", read_error),
                })),
            },
        };

        let mut buf = Vec::with_capacity(response.encoded_len());
        // Unwrap is safe, since we have reserved sufficient capacity in the vector.
        response.encode(&mut buf).unwrap();
        Ok(full(Bytes::from(buf)))
    }
}

impl Handle<PutRequest> for RisDbSvc {
    async fn handle(
        &self,
        input: PutRequest,
    ) -> Result<BoxBody<Bytes, hyper::Error>, hyper::Error> {
        let propose_value_output = self
            .risdb
            .database
            .propose_value(ProposeValueInput {
                request_id: input.request_id.to_owned(),
                values: input
                    .values
                    .into_iter()
                    .map(|this_crate_value| this_crate_value.into())
                    .collect(),
            })
            .await;

        let response = match propose_value_output {
            Ok(_output) => PutResponse {
                request_id: input.request_id,
                response: Some(put_response::Response::Success(PutSuccess {})),
            },
            Err(write_error) => PutResponse {
                request_id: input.request_id.to_owned(),
                response: Some(put_response::Response::Failure(PutFailure {
                    reason: format!("Failed to put: {:?}", write_error),
                })),
            },
        };

        let mut buf = Vec::with_capacity(response.encoded_len());
        // Unwrap is safe, since we have reserved sufficient capacity in the vector.
        response.encode(&mut buf).unwrap();
        Ok(full(Bytes::from(buf)))
    }
}

trait Respond {
    async fn respond<T>(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error>
    where
        T: prost::Message + Default,
        Self: Handle<T>;
}

impl Respond for RisDbSvc {
    async fn respond<T>(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error>
    where
        T: prost::Message + Default,
        Self: Handle<T>,
    {
        let bytes = match accept_body(req).await {
            // Early return if the body sent is too large
            Err(AcceptBodyError::BodyTooLarge) => return Ok(payload_too_large()),
            Err(AcceptBodyError::FailedToConsume(err)) => Err(err),
            Ok(bytes) => Ok(bytes),
        }?;

        let request = match T::decode(bytes) {
            Err(err) => return Ok(failed_to_decode(err)),
            Ok(get_request) => get_request,
        };

        let response = self.handle(request).await?;

        Ok(Response::new(response))
    }
}

enum AcceptBodyError {
    BodyTooLarge,
    FailedToConsume(hyper::Error),
}

impl From<hyper::Error> for AcceptBodyError {
    fn from(err: hyper::Error) -> Self {
        AcceptBodyError::FailedToConsume(err)
    }
}

async fn accept_body(req: Request<Incoming>) -> Result<Bytes, AcceptBodyError> {
    // Protect our server from massive bodies.
    let upper = req.body().size_hint().upper().unwrap_or(u64::MAX);
    if upper > 1024 * 64 {
        return Err(AcceptBodyError::BodyTooLarge);
    }

    // Await the whole body to be collected into a single `Bytes`...
    let whole_body = req.collect().await?.to_bytes();
    Ok(whole_body)
}

fn payload_too_large() -> Response<BoxBody<Bytes, hyper::Error>> {
    // Early return if the body sent is too large
    let mut resp = Response::new(full("Body too big"));
    *resp.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
    resp
}

fn failed_to_decode(err: DecodeError) -> Response<BoxBody<Bytes, hyper::Error>> {
    let mut resp = Response::new(full(format!(
        "Failed to decode the provided input: {:?}",
        err
    )));
    *resp.status_mut() = StatusCode::BAD_REQUEST;
    resp
}

#[derive(Debug, Clone)]
pub struct Logger<S> {
    inner: S,
}
impl<S> Logger<S> {
    pub fn new(inner: S) -> Self {
        Logger { inner }
    }
}
type Req = Request<Incoming>;

impl<S> Service<Req> for Logger<S>
where
    S: Service<Req>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;
    fn call(&self, req: Req) -> Self::Future {
        info!("processing request: {} {}", req.method(), req.uri().path());
        self.inner.call(req)
    }
}

// We create some utility functions to make Empty and Full bodies
// fit our broadened Response body type.
fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}
fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}
