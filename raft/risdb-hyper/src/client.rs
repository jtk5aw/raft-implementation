use crate::helper::error;
use crate::structs::{GetRequest, GetResponse, PutRequest, PutResponse, PutSuccess};
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
use tracing::debug;

// Errors
#[derive(Debug)]
pub enum ClientBuilderError {
    MissingRootCertPath,
    MissingBaseUri(String),
    InvalidActionUri(http::Error),
    ClientBuilderIoError(std::io::Error),
}

impl From<std::io::Error> for ClientBuilderError {
    fn from(err: Error) -> Self {
        ClientBuilderError::ClientBuilderIoError(err)
    }
}

impl From<http::Error> for ClientBuilderError {
    fn from(err: http::Error) -> Self {
        ClientBuilderError::InvalidActionUri(err)
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

// Structs
pub struct RisDbClient {
    /// Hyper client used to actually make requests
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    /// Struct containing URIs that can be called
    endpoints: Endpoints,
}

pub struct Endpoints {
    /// Authority for all the provided endpoints
    authority: Authority,
    /// Get URI
    get_uri: Uri,
    /// Put URI
    put_uri: Uri,
}

pub struct ClientBuilder {
    /// Root CA Cert path
    ca_cert_path: Option<PathBuf>,
    /// Base URI to make requests against
    base_uri: Option<Uri>,
}

// Traits
/// TODO: Update this to do response too once that's implemented cause there will be duplicate code there.
pub trait CreateRequest<T>
where
    T: prost::Message,
{
    fn create_request(&self, input: T) -> Result<Request<Full<Bytes>>, http::Error>;

    fn write_to_buffer(input: T) -> Bytes {
        let mut buf = Vec::with_capacity(input.encoded_len());
        // Unwrap is safe, since we have reserved sufficient capacity in the vector.
        input.encode(&mut buf).unwrap();
        Bytes::from(buf)
    }
}

pub trait Get: CreateRequest<GetRequest> {
    fn get(&self, keys: GetRequest)
        -> impl Future<Output = Result<GetResponse, RisDbError>> + Send;
}

pub trait Put: CreateRequest<PutRequest> {
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
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_only()
            .enable_http2()
            .build();

        let endpoints = Endpoints::initialize(self.base_uri)?;

        // TODO: Figure out how this TokioExecutor plays with the Raft one. Is it even a real Tokio Executor?
        // TODO: One hyper_rustls uses the new hyper stuff (not the legacy client) upgrade it
        let client = Client::builder(TokioExecutor::new())
            .http2_only(true)
            .build(https);

        Ok(RisDbClient { client, endpoints })
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
    authority: Authority,
    scheme: Scheme,
}

impl ParsedUri {
    fn parse_base_uri(uri: Uri) -> Result<ParsedUri, ClientBuilderError> {
        let parts = uri.into_parts();
        if let Some(path_and_query) = parts.path_and_query {
            if !path_and_query.as_str().eq("/") {
                debug!("Bad Path/Query: {:?}", &path_and_query);
                return Err(ClientBuilderError::MissingBaseUri(
                    "Provided Base URI can not have a path and query section".to_string(),
                ));
            }
        }

        let authority = parts
            .authority
            .ok_or(ClientBuilderError::MissingBaseUri(
                "Provided Base URI has no Authority".to_string(),
            ))?
            .to_owned();
        let scheme = parts
            .scheme
            .ok_or(ClientBuilderError::MissingBaseUri(
                "Provided Base URI must have a scheme".to_string(),
            ))?
            .to_owned();

        Ok(ParsedUri { authority, scheme })
    }
}

impl Endpoints {
    fn initialize(uri: Option<Uri>) -> Result<Endpoints, ClientBuilderError> {
        let uri_to_parse = uri.ok_or(ClientBuilderError::MissingBaseUri(
            "Required to provide a Base URI".to_string(),
        ))?;
        let parsed_uri = ParsedUri::parse_base_uri(uri_to_parse)?;

        let authority = parsed_uri.authority.clone();

        let get_uri = Uri::builder()
            .scheme(parsed_uri.scheme.clone())
            .authority(parsed_uri.authority.clone())
            .path_and_query("/get")
            .build()?;

        let put_uri = Uri::builder()
            .scheme(parsed_uri.scheme.clone())
            .authority(parsed_uri.authority.clone())
            .path_and_query("/put")
            .build()?;

        Ok(Endpoints {
            authority,
            get_uri,
            put_uri,
        })
    }
}

// Note to future self: I went ahead and left these CreateRequestfs with some duplicate code as I could very easily
// see how these requests are generated diverging in headers added. Decision that could definitely
// be revisited later.

impl CreateRequest<GetRequest> for RisDbClient {
    fn create_request(&self, input: GetRequest) -> Result<Request<Full<Bytes>>, http::Error> {
        let bytes = Self::write_to_buffer(input);

        Request::post(self.endpoints.get_uri.clone())
            .header(hyper::header::HOST, self.endpoints.authority.as_str())
            .body(Full::from(bytes))
    }
}

impl CreateRequest<PutRequest> for RisDbClient {
    fn create_request(&self, input: PutRequest) -> Result<Request<Full<Bytes>>, http::Error> {
        let bytes = Self::write_to_buffer(input);

        Request::post(self.endpoints.put_uri.clone())
            .header(hyper::header::HOST, self.endpoints.authority.as_str())
            .body(Full::from(bytes))
    }
}

impl Get for RisDbClient {
    async fn get(&self, keys: GetRequest) -> Result<GetResponse, RisDbError> {
        let req = self.create_request(keys)?;

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
        let response = GetResponse::decode(bytes)?;

        Ok(response)
    }
}

impl Put for RisDbClient {
    async fn put(&self, values: PutRequest) -> Result<PutResponse, RisDbError> {
        let req = self.create_request(values)?;

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

        // TODO TODO TODO: Make the server send a PutResponse and decode that
        let response = PutResponse::decode(bytes)?;

        Ok(response)
    }
}
