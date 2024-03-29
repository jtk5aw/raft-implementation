use std::net::SocketAddr;

use tokio::{try_join, task::JoinHandle};
use tonic::transport::Server;

use crate::raft::node::{RaftImpl, HeartbeatError, SetupError};
use crate::raft_grpc::raft_internal_server::RaftInternalServer;
use crate::risdb::ris_db_server::RisDbServer;
use crate::risdb_impl::{RisDbImpl, RisDbSetupError};

// Errors
pub enum StartUpError {
    FailedToSetup(SetupError),
    FailedToStartServer(tonic::transport::Error),
    FailedToLocallyConnect(RisDbSetupError),
    FailedToHeartbeat(HeartbeatError),
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
        addr: SocketAddr,
        peers: Vec<String>
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
    ) -> Result<(), StartUpError>;

}

#[tonic::async_trait]
impl RisDbSetup for RisDb {
    #[tracing::instrument]
    async fn startup(
        self,
        addr: SocketAddr,
        peers: Vec<String>
    ) -> Result<(), Vec<String>> {

        let raft = RaftImpl::new(addr);
        let risdb = RisDbImpl::new(addr);

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
            )
        );
        let heartbeat_handle = tokio::spawn(
            RaftImpl::heartbeat(
                addr.to_string(),
                heartbeat_peer_connections,
                heartbeat_raft_state,
                heartbeat_volativle_raft_state,
            )
        );
        let peer_connections_handle = tokio::spawn(
            RaftImpl::connect_to_peers(addr,
                peers,
                connect_to_peers_peer_connections,
                connect_to_peers_raft_stable_state
            )
        );
        let loopback_connect_handle = tokio::spawn(
            loopback_raft_client.init_client(
                addr
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
                }
            }
        }

        Ok(())
    }

    async fn serve_wrapper(
        self,
        raft: RaftImpl,
        risdb: RisDbImpl,
    ) -> Result<(), StartUpError> {
        // TODO: This feels odd, shouldn't need to do this
        let addr = raft.addr.clone();

        let _ = Server::builder()
            .add_service(RaftInternalServer::new(raft))
            .add_service(RisDbServer::new(risdb))
            .serve(addr)
            .await?;

        Ok(())
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


