use crate::helper::error;
use crate::items::{GetRequest, PutRequest, PutResponse};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::http::uri::{Authority, Scheme};
use hyper::{http, Request, Uri};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use prost::{DecodeError, Message};
use rustls::{ClientConfig, RootCertStore};
use std::future::Future;
use std::io::Error;
use std::path::PathBuf;
use std::{fs, io};
use tokio::io::AsyncWriteExt;

// Errors
#[derive(Debug)]
pub enum ClientBuilderError {
    MissingRootCertPath,
    MissingBaseUri(String),
    ClientBuilderIoError(std::io::Error),
}

impl From<std::io::Error> for ClientBuilderError {
    fn from(err: Error) -> Self {
        ClientBuilderError::ClientBuilderIoError(err)
    }
}

#[derive(Debug)]
pub enum RisDbError {
    FailedToConstructRequest(http::Error),
    FailedToMakeRequest(std::io::Error),
    FailedToReadResponse(hyper::Error),
    FailedToDecodeResponse(DecodeError),
}

impl From<http::Error> for RisDbError {
    fn from(err: http::Error) -> Self {
        RisDbError::FailedToConstructRequest(err)
    }
}

impl From<std::io::Error> for RisDbError {
    fn from(err: Error) -> Self {
        RisDbError::FailedToMakeRequest(err)
    }
}

impl From<hyper::Error> for RisDbError {
    fn from(err: hyper::Error) -> Self {
        RisDbError::FailedToReadResponse(err)
    }
}

impl From<DecodeError> for RisDbError {
    fn from(err: DecodeError) -> Self {
        RisDbError::FailedToDecodeResponse(err)
    }
}

// Public Structs
pub struct RisDbClient {
    /// Hyper client used to actually make requests
    // TODO: Consider changing this String type to something else. Maybe bytes
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    /// Base URI to make requests against
    base_uri: Uri,
    /// Authority derived from the base_uri
    authority: Authority,
    /// Scheme derived from the base_uri
    scheme: Scheme,
}

pub struct ClientBuilder {
    /// Root CA Cert path
    ca_cert_path: Option<PathBuf>,
    /// Base URI to make requests against
    base_uri: Option<Uri>,
}

// Traits
pub trait Get {
    fn get(&self, keys: GetRequest) -> impl Future<Output = Result<GetRequest, RisDbError>> + Send;
}

pub trait Put {
    fn put(
        &self,
        values: PutRequest,
    ) -> impl Future<Output = Result<PutResponse, RisDbError>> + Send;
}

// Impls
impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            ca_cert_path: None,
            base_uri: None,
        }
    }

    pub fn with_root_cert_path(mut self, root_cert_path: PathBuf) -> Self {
        self.ca_cert_path = Some(root_cert_path);
        self
    }

    pub fn with_base_uri(mut self, base_uri: Uri) -> Self {
        self.base_uri = Some(base_uri);
        self
    }

    pub async fn build(self) -> Result<RisDbClient, ClientBuilderError> {
        let ca_cert_path = self
            .ca_cert_path
            .ok_or(ClientBuilderError::MissingRootCertPath)?;
        let tls = load_root_cert(&ca_cert_path)?;
        let parsed_uri = ParsedUri::parse_base_uri(self.base_uri)?;
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_only()
            .enable_http2()
            .build();

        // TODO: Figure out how this TokioExecutor plays with the Raft one. Is it even a real Tokio Executor?
        // TODO: One hyper_rustls uses the new hyper stuff (not the legacy client) upgrade t
        let client = Client::builder(TokioExecutor::new())
            .http2_only(true)
            .build(https);

        Ok(RisDbClient {
            client,
            base_uri: parsed_uri.base_uri,
            authority: parsed_uri.authority,
            scheme: parsed_uri.scheme,
        })
    }
}

fn load_root_cert(path: &PathBuf) -> Result<ClientConfig, std::io::Error> {
    let f = fs::File::open(path).map_err(|e| error(format!("failed to open {:?}: {}", path, e)))?;
    let mut rd = io::BufReader::new(f);

    let certs = rustls_pemfile::certs(&mut rd).collect::<Result<Vec<_>, _>>()?;
    let mut roots = RootCertStore::empty();
    let result = roots.add_parsable_certificates(certs);
    println!("added: {:?}, ignored: {:?}", result.0, result.1);
    println!("roots: {:?}", roots);
    // TLS client config using the custom CA store for lookups
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

struct ParsedUri {
    base_uri: Uri,
    authority: Authority,
    scheme: Scheme,
}

impl ParsedUri {
    fn parse_base_uri(base_uri: Option<Uri>) -> Result<ParsedUri, ClientBuilderError> {
        let base_uri = base_uri.ok_or(ClientBuilderError::MissingBaseUri(
            "Required to provide a Base URI".to_string(),
        ))?;
        let authority = base_uri
            .authority()
            .ok_or(ClientBuilderError::MissingBaseUri(
                "Provided Base URI has no Authority".to_string(),
            ))?
            .to_owned();
        let scheme = base_uri
            .scheme()
            .ok_or(ClientBuilderError::MissingBaseUri(
                "Provided Base URI must have a scheme".to_string(),
            ))?
            .to_owned();

        Ok(ParsedUri {
            base_uri,
            authority,
            scheme,
        })
    }
}

impl Get for RisDbClient {
    async fn get(&self, keys: GetRequest) -> Result<GetRequest, RisDbError> {
        let mut buf = Vec::with_capacity(keys.encoded_len());
        // Unwrap is safe, since we have reserved sufficient capacity in the vector.
        keys.encode(&mut buf).unwrap();
        let bytes = Bytes::from(buf);

        // TODO: Make it so this is generated once rather than on every request
        let get_uri = Uri::builder()
            .scheme(self.scheme.clone())
            .authority(self.authority.clone())
            .path_and_query("/echo")
            .build()?;

        let req = Request::post(get_uri)
            .header(hyper::header::HOST, self.authority.as_str())
            .body(Full::from(bytes))?;

        let mut result = self
            .client
            .request(req)
            .await
            .map_err(|e| error(format!("Could not get: {:?}", e)))?;

        let mut buf: Vec<u8> = Vec::with_capacity(20);
        while let Some(next) = result.frame().await {
            let frame = next?;
            if let Some(chunk) = frame.data_ref() {
                // TODO: This might be returned a confusing error fix if neccessary
                buf.write_all(chunk).await?;
            }
        }
        let bytes = Bytes::from(buf);

        // TODO TODO TODO: Make the server send a GEtResposne and decode that
        let response = GetRequest::decode(bytes)?;

        Ok(response)
    }
}

impl Put for RisDbClient {
    async fn put(&self, values: PutRequest) -> Result<PutResponse, RisDbError> {
        todo!()
    }
}
