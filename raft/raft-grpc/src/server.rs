use std::fs::{create_dir_all, File};
use std::io::{Error as IoError};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use hyper::server::conn::Http;
use tower_http::ServiceBuilderExt;
use tokio::{try_join, task::JoinHandle, signal};
use tokio::net::TcpListener;
use tokio_rustls::rustls::ServerConfig;
use tonic::transport::{Server, ServerTlsConfig};
use tokio_rustls::rustls::{Error as RustTlsError};
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::TlsAcceptor;

use crate::raft::node::{RaftImpl, HeartbeatError, SetupError};
use crate::raft_grpc::raft_internal_server::RaftInternalServer;
use crate::risdb::ris_db_server::RisDbServer;
use crate::risdb_impl::{RisDbImpl, RisDbSetupError};

#[derive(Debug)]
pub struct ServerArgs {
    pub risdb_addr: SocketAddr,
    pub raft_addr: SocketAddr,
    pub key_dir: PathBuf,
    pub peer_args: Vec<PeerArgs>
}

#[derive(Debug)]
pub struct PeerArgs {
    pub addr: String,
    pub key_dir: PathBuf
}

// Errors
pub enum StartUpError {
    FailedToSetup(SetupError),
    FailedToStartServer(tonic::transport::Error),
    FailedToLocallyConnect(RisDbSetupError),
    FailedToHeartbeat(HeartbeatError),
    FailedToReadSelfSignedCert(IoError),
    FailedToSetUpTls(RustTlsError),
    CustomError(String),
}

impl From<SetupError> for StartUpError {
    fn from(err: SetupError) -> StartUpError {
        StartUpError::FailedToSetup(err)
    }
}

impl From<tonic::transport::Error> for StartUpError {
    fn from(err: tonic::transport::Error) -> StartUpError {
        StartUpError::FailedToStartServer(err)
    }
}

impl From<RisDbSetupError> for StartUpError {
    fn from(err: RisDbSetupError) -> StartUpError {
        StartUpError::FailedToLocallyConnect(err)
    }
}

impl From<HeartbeatError> for StartUpError {
    fn from(err: HeartbeatError) -> StartUpError {
        StartUpError::FailedToHeartbeat(err)
    }
}

impl From<IoError> for StartUpError {
    fn from(err: IoError) -> StartUpError {
        StartUpError::FailedToReadSelfSignedCert(err)
    }
}

impl From<RustTlsError> for StartUpError {
    fn from(err: RustTlsError) -> StartUpError {
        StartUpError::FailedToSetUpTls(err)
    }
}


impl From<String> for StartUpError {
    fn from(err: String) -> StartUpError {
        StartUpError::CustomError(err)
    }
}


// Struct for setup
#[derive(Debug)]
pub struct RisDb {}


// Public traits
#[tonic::async_trait]
pub trait RisDbSetup {
    /**
     * Starts up this raft server and connects it to all of its peers.
     *
     * If it fails to connect to peers it will return an error.
     *
     * `Result`:
     * `Ok(())` - Server has shut down
     * `SetupError(Vec<String>)` - Server failed to connect to peers and never began serving requests.
     *  List of peers it failed to connect to are returned
     */
    async fn startup(
        self,
        server_args: ServerArgs
    ) -> Result<(), Vec<String>>;

}

#[tonic::async_trait]
impl RisDbSetup for RisDb {
    #[tracing::instrument]
    async fn startup(
        self,
        server_args: ServerArgs
    ) -> Result<(), Vec<String>> {

        let raft = RaftImpl::new(server_args.raft_addr);
        let risdb = RisDbImpl::new(server_args.risdb_addr);

        let heartbeat_peer_connections = raft.peer_connections.clone();
        let heartbeat_raft_state = raft.state.clone();
        let heartbeat_volativle_raft_state = raft.volatile_state.clone();

        let connect_to_peers_peer_connections = raft.peer_connections.clone();
        let connect_to_peers_raft_stable_state = raft.state.clone();

        let loopback_raft_client = risdb.raft_client.clone();

        // Starts the server and a "setup" thread to run simultaneously.
        // NOTE: Task are spawned manually so that an error can have ::from() called to map to
        // a StartUpError.
        // TODO: Clean this up or abstract it into its own function
        let risdb_handle = tokio::spawn(
            serve_risdb(
                risdb,
                server_args.key_dir,
            )
        );
        let raft_handle = tokio::spawn(
            serve_raft(
                raft
            )
        );
        let heartbeat_handle = tokio::spawn(
            RaftImpl::heartbeat(
                server_args.raft_addr.to_string(),
                heartbeat_peer_connections,
                heartbeat_raft_state,
                heartbeat_volativle_raft_state,
            )
        );
        let peer_connections_handle = tokio::spawn(
            RaftImpl::connect_to_peers(server_args.raft_addr,
                server_args.peer_args,
                connect_to_peers_peer_connections,
                connect_to_peers_raft_stable_state
            )
        );
        let loopback_connect_handle = tokio::spawn(
            loopback_raft_client.init_client(
                server_args.raft_addr
            )
        );

        let result = try_join!(
            // Should never return while the server is live
            flatten(risdb_handle),
            flatten(raft_handle),
            flatten(heartbeat_handle),
            // Should return relatively quickly after some setup occurs
            flatten(peer_connections_handle),
            flatten(loopback_connect_handle)
        );

        if result.is_err() {
            match result.err().unwrap() {
                StartUpError::FailedToSetup(failures) => {
                    tracing::error!("Failed to connect to peers: {:?}", failures);
                },
                StartUpError::FailedToStartServer(err) => {
                    tracing::error!("Failed to start the server: {:?}", err);
                },
                StartUpError::FailedToLocallyConnect(err) => {
                    tracing::error!("Failed to connect to the local raft server: {:?}", err);
                },
                StartUpError::FailedToHeartbeat(err) => {
                    tracing::error!("Failed to heartbeat: {:?}", err);
                },
                StartUpError::CustomError(err) => {
                    tracing::error!("Most likely a concurrency issue: {:?}", err);
                },
                StartUpError::FailedToReadSelfSignedCert(err) => {
                    tracing::error!("Failed to read a self signed cert to be used for TLS: {:?}" , err);
                },
                StartUpError::FailedToSetUpTls(err) => {
                    tracing::error!("Failed to setup TLS: {:?}" , err);
                },
            }
        }

        Ok(())
    }
}

async fn serve_raft(
    raft: RaftImpl
) -> Result<(), StartUpError> {
    let addr = raft.addr.clone();

    Server::builder()
        .add_service(RaftInternalServer::new(raft))
        .serve(addr)
        .await?;

    Ok(())
}

async fn serve_risdb(
    risdb: RisDbImpl,
    key_dir: PathBuf,
) -> Result<(), StartUpError> {
    // TODO: This feels odd, shouldn't need to do this
    let addr = risdb.addr.clone();

    let certs = {
        let fd = std::fs::File::open(key_dir.join("server.crt"))?;
        let mut buf = std::io::BufReader::new(&fd);
        rustls_pemfile::certs(&mut buf)
            .collect::<Result<Vec<_>, _>>()?
    };
    let key = {
        let fd = std::fs::File::open(key_dir.join("priv.key"))?;
        let mut buf = std::io::BufReader::new(&fd);
        rustls_pemfile::private_key(&mut buf)?
            .unwrap()
    };

    let mut tls = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    tls.alpn_protocols = vec![b"h2".to_vec()];

    let svc = Server::builder()
        .add_service(RisDbServer::new(risdb))
        .into_service();

    let mut http = Http::new();
    http.http2_only(true);

    let listener = TcpListener::bind(addr).await?;
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls));

    loop {
        let (conn, addr) = match listener.accept().await {
            Ok(incoming) => incoming,
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
                continue;
            }
        };

        let http = http.clone();
        let tls_acceptor = tls_acceptor.clone();
        let svc = svc.clone();

        tokio::spawn(async move {
            let mut certificates = Vec::new();

            let conn = tls_acceptor
                .accept_with(conn, |info| {
                    if let Some(certs) = info.peer_certificates() {
                        for cert in certs {
                            certificates.push(cert.clone());
                        }
                    }
                })
                .await
                .unwrap();

            let svc = tower::ServiceBuilder::new()
                .service(svc);

            http.serve_connection(conn, svc).await.unwrap();
        });
    }
}


async fn wait() {
    println!("server listening");
    signal::ctrl_c().await.expect("TODO: panic message");
    println!("shutdown complete");
}

async fn flatten<T, D>(
    handle: JoinHandle<Result<T, D>>
) -> Result<T, StartUpError> where StartUpError: From<D> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(StartUpError::from(err)),
        Err(_) => Err(StartUpError::CustomError("handling failed".to_owned())),
    }
}


