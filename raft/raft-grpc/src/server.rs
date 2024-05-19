use std::io::{Error as IoError};
use std::net::SocketAddr;

use tokio::{try_join, task::JoinHandle};
use tonic::transport::Server;

use crate::raft::node::{RaftImpl, HeartbeatError, SetupError};
use crate::raft_grpc::raft_internal_server::RaftInternalServer;
use crate::risdb::ris_db_server::RisDbServer;
use crate::risdb_impl::{RisDbImpl, RisDbSetupError};

#[derive(Debug)]
pub struct ServerArgs {
    pub risdb_addr: SocketAddr,
    pub raft_addr: SocketAddr,
    pub peer_args: Vec<PeerArgs>
}

#[derive(Debug)]
pub struct PeerArgs {
    pub addr: String,
}

// Errors
pub enum StartUpError {
    FailedToSetup(SetupError),
    FailedToStartServer(tonic::transport::Error),
    FailedToLocallyConnect(RisDbSetupError),
    FailedToHeartbeat(HeartbeatError),
    FailedToReadSelfSignedCert(IoError),
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
) -> Result<(), StartUpError> {
    // TODO: This feels odd, shouldn't need to do this
    let addr = risdb.addr.clone();

    Server::builder()
        .add_service(RisDbServer::new(risdb))
        .serve(addr)
        .await?;

    Ok(())
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


