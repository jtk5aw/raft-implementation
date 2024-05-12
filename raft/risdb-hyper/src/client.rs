use std::{fs, io};
use hyper::{Request, Uri};
use hyper::client::conn::http2::handshake;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpStream;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncWriteExt as _};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct GetKeysRequest {
    keys: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // This is where we will setup our HTTP client requests.

    // Load root cert
    let tls = load_root_cert("/Users/jacksonkennedy/Documents/raft-implementation/raft/risdb-hyper/src/ca.cert")
        .expect("Should create ClientConfig using provided root cert");

    // Parse our URL...
    let url = "https://localhost:3000/echo".parse::<hyper::Uri>()?;

    // Get the host and the port
    let host = url.host().expect("uri has no host");
    let port = url.port_u16().unwrap_or(3000);

    let address = format!("{}:{}", host, port);

    // Open a TCP connection to the remote host
    let stream = TcpStream::connect(address).await?;

    // Use an adapter to access something implementing `tokio::io` traits as if they implement
    // `hyper::rt` IO traits.
    let io = TokioIo::new(stream);

    // TLS config
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_only()
        .enable_http2()
        .build();

    // The authority of our URL will be the hostname of the httpbin remote
    let authority = url.authority().unwrap().clone();

    // TODO TODO TODO: Condense this into one
    let mut res = if true {
        let client: Client<_, String> = Client::builder(TokioExecutor::new())
            .build(https);

        // Create a GetKeysRequest request
        let request_body = GetKeysRequest {
            keys: vec!["key1".into(), "key2".into()]
        };
        let req = Request::post(url)
            .header(hyper::header::HOST, authority.as_str())
            .body(
                serde_json::to_string(&request_body)?
            )?;

        client
            .request(req)
            .await
            .map_err(|e| error(format!("Could not get: {:?}", e)))?
    } else {
        // Create the Hyper client
        let (mut sender, conn) =
            handshake(TokioExecutor::new(), io).await?;

        // Spawn a task to poll the connection, driving the HTTP state
        tokio::task::spawn(async move {
            if let Err(err) = conn.await {
                println!("Connection failed: {:?}", err);
            }
        });

        // Create a GetKeysRequest request
        let request_body = GetKeysRequest {
            keys: vec!["key1".into(), "key2".into()]
        };
        let req = Request::post(url)
            .header(hyper::header::HOST, authority.as_str())
            .body(
                serde_json::to_string(&request_body)?
            )?;

        // Await the response...
        sender.send_request(req).await?
    };

    println!("Response status: {}", res.status());

    // Stream the body, writing each frame to a buffer
    let mut buf: Vec<u8> = Vec::with_capacity(20);
    while let Some(next) = res.frame().await {
        let frame = next?;
        if let Some(chunk) = frame.data_ref() {
            buf.write_all(chunk).await?;
        }
    }
    let test: GetKeysRequest = serde_json::from_slice(buf.as_slice())?;

    println!("{:?}", test);

    Ok(())
}

fn load_root_cert(path: &str) -> Result<ClientConfig, std::io::Error> {
    let f = fs::File::open(path)
        .map_err(|e| error(format!("failed to open {}: {}", path, e)))?;
    let mut rd = io::BufReader::new(f);

    let certs = rustls_pemfile::certs(&mut rd).collect::<Result<Vec<_>, _>>()?;
    let mut roots = RootCertStore::empty();
    let result = roots.add_parsable_certificates(certs);
    println!("added: {:?}, ignored: {:?}", result.0, result.1);
    println!("roots: {:?}", roots);
    // TLS client config using the custom CA store for lookups
    Ok(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    )
}

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

// NOTE: This part is only needed for HTTP/2. HTTP/1 doesn't need an executor.
//
// Since the Server needs to spawn some background tasks, we needed
// to configure an Executor that can spawn !Send futures...
#[derive(Clone, Copy, Debug)]
struct LocalExec;

impl<F> hyper::rt::Executor<F> for LocalExec
    where
        F: std::future::Future + 'static, // not requiring `Send`
{
    fn execute(&self, fut: F) {
        // This will spawn into the currently running `LocalSet`.
        tokio::task::spawn_local(fut);
    }
}