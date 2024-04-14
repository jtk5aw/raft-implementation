use std::fs::{create_dir_all, File};
use std::io::{Write, Error as IoError};
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::server::conn::Http;
use tower_http::ServiceBuilderExt;
use tokio::{try_join, task::JoinHandle};
use tokio::net::TcpListener;
use tokio_rustls::rustls::{Certificate, PrivateKey, ServerConfig};
use tonic::transport::{Identity, Server, ServerTlsConfig};
use tokio_rustls::rustls::{Error as RustTlsError};
use tokio_rustls::TlsAcceptor;

use crate::raft::node::{RaftImpl, HeartbeatError, SetupError};
use crate::raft_grpc::raft_internal_server::RaftInternalServer;
use crate::risdb::ris_db_server::RisDbServer;
use crate::risdb_impl::{RisDbImpl, RisDbSetupError};

#[derive(Debug)]
pub struct ServerArgs {
    pub addr: SocketAddr,
    pub key_dir: String,
    pub peer_args: Vec<PeerArgs>
}

#[derive(Debug)]
pub struct PeerArgs {
    pub addr: String,
    pub key_dir: String
}

#[derive(Debug)]
struct ConnInfo {
    addr: SocketAddr,
    certificates: Vec<Certificate>,
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

    /**
     * Wraps the `serve` function of tonic so that a try_join can be done on it
     * and the setup peers call
     *
     * `Result`:
     * `Ok(())` - Server has shut down
     * `SetupError(tonic::transport::Error)` - Server failed to start
     */
    async fn serve_wrapper(
        self,
        raft: RaftImpl,
        risdb: RisDbImpl,
        key_dir: String,
    ) -> Result<(), StartUpError>;

}

#[tonic::async_trait]
impl RisDbSetup for RisDb {
    #[tracing::instrument]
    async fn startup(
        self,
        server_args: ServerArgs
    ) -> Result<(), Vec<String>> {

        let raft = RaftImpl::new(server_args.addr);
        let risdb = RisDbImpl::new(server_args.addr);

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
        let serve_wrapper_handle = tokio::spawn(
            self.serve_wrapper(
                raft,
                risdb,
                server_args.key_dir,
            )
        );
        let heartbeat_handle = tokio::spawn(
            RaftImpl::heartbeat(
                server_args.addr.to_string(),
                heartbeat_peer_connections,
                heartbeat_raft_state,
                heartbeat_volativle_raft_state,
            )
        );
        let peers = server_args.peer_args.iter()
            .map(|peer_arg| peer_arg.addr.to_string())
            .collect();
        let peer_connections_handle = tokio::spawn(
            RaftImpl::connect_to_peers(server_args.addr,
                peers,
                connect_to_peers_peer_connections,
                connect_to_peers_raft_stable_state
            )
        );
        let loopback_connect_handle = tokio::spawn(
            loopback_raft_client.init_client(
                server_args.addr
            )
        );

        let result = try_join!(
            // Should never return while the server is live
            flatten(serve_wrapper_handle),
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

    async fn serve_wrapper(
        self,
        raft: RaftImpl,
        risdb: RisDbImpl,
        key_dir: String,
    ) -> Result<(), StartUpError> {
        // TODO: This feels odd, shouldn't need to do this
        let addr = raft.addr.clone();

        let certs = {
            let fd = std::fs::File::open(format!("{}/{}", key_dir, "server.crt"))?;
            let mut buf = std::io::BufReader::new(&fd);
            rustls_pemfile::certs(&mut buf)?
                .into_iter()
                .map(Certificate)
                .collect()
        };
        let key = {
            let fd = std::fs::File::open(format!("{}/{}", key_dir, "priv.key"))?;
            let mut buf = std::io::BufReader::new(&fd);
            rustls_pemfile::pkcs8_private_keys(&mut buf)?
                .into_iter()
                .map(PrivateKey)
                .next()
                .unwrap()

        };

        let mut tls = ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        tls.alpn_protocols = vec![b"h2".to_vec()];

        let svc = Server::builder()
            .add_service(RisDbServer::new(risdb))
            .add_service(RaftInternalServer::new(raft))
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
                    .add_extension(Arc::new(ConnInfo { addr, certificates }))
                    .service(svc);

                http.serve_connection(conn, svc).await.unwrap();
            });
        }
    }
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


