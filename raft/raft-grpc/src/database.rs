use std::net::SocketAddr;
use std::sync::Arc;

use tokio::runtime::{Builder, Runtime};
use tokio::{task::JoinHandle, try_join};
use tonic::transport::Server;

use crate::raft::node::{Database, HeartbeatError, RaftImpl, SetupError};
use crate::structs::raft_internal_server::RaftInternalServer;

#[derive(Debug)]
pub struct ServerArgs {
    pub risdb_addr: SocketAddr,
    pub raft_addr: SocketAddr,
    pub peer_args: Vec<PeerArgs>,
}

#[derive(Debug)]
pub struct PeerArgs {
    pub addr: String,
}

// Errors
pub enum StartUpError {
    FailedToSetup(SetupError),
    FailedToStartServer(tonic::transport::Error),
    FailedToHeartbeat(HeartbeatError),
    FailedToStartTokioRuntime(std::io::Error),
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

impl From<HeartbeatError> for StartUpError {
    fn from(err: HeartbeatError) -> StartUpError {
        StartUpError::FailedToHeartbeat(err)
    }
}

impl From<std::io::Error> for StartUpError {
    fn from(err: std::io::Error) -> StartUpError {
        StartUpError::FailedToStartTokioRuntime(err)
    }
}

impl From<String> for StartUpError {
    fn from(err: String) -> StartUpError {
        StartUpError::CustomError(err)
    }
}

// Struct for setup
#[derive(Debug)]
pub struct RisDb {
    /// Tokio Runtime used by the database
    runtime: Runtime,
    /// Representation of the database that will server reads/writes
    pub database: Arc<Database>,
}

// Public traits

pub trait RisDbImpl {
    /// Performs initial setup to create an implementation.
    /// Will initialize the Tokio runtime for executing all
    /// tasks.
    ///
    /// socket_addr: The address at which this raft node will listen for incoming requests
    fn new(socket_addr: SocketAddr) -> Self;

    /// Retrieves a handle on the data store backing this database.
    /// All interaction with the

    ///
    /// Starts up this raft server and connects it to all of its peers.
    /// If it fails to connect to peers it will return an error.
    ///
    /// `Result`:
    /// `Ok(())` - Server has shut down
    /// `Err(StartupError)` - Server failed to start or irrecoverably failed a core function.
    fn startup(&self, server_args: ServerArgs) -> JoinHandle<Result<(), StartUpError>>;
}

impl RisDbImpl for RisDb {
    #[tracing::instrument]
    fn new(addr: SocketAddr) -> Self {
        Self {
            runtime: Builder::new_multi_thread()
                // Note: I pulled this number out of thin air :shrug:.
                // I expect 2 cores so 2 worker threads might be better but need to test it
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap(),
            database: Arc::new(Database::new(addr)),
        }
    }

    #[tracing::instrument]
    fn startup(&self, server_args: ServerArgs) -> JoinHandle<Result<(), StartUpError>> {
        // Get a handle to the underlying database struct so only that is moved and not self
        let database = self.database.clone();
        let addr = database.addr.clone();

        // Spawn the parent task running the database
        self.runtime.spawn(async move {
            let raft = RaftImpl::new(database);

            let heartbeat_peer_connections = raft.inner.peer_connections.clone();
            let heartbeat_raft_state = raft.inner.state.clone();
            let heartbeat_volatile_raft_state = raft.inner.volatile_state.clone();

            let connect_to_peers_peer_connections = raft.inner.peer_connections.clone();
            let connect_to_peers_raft_stable_state = raft.inner.state.clone();

            // NOTE: Task are spawned manually so that all this work is done on separate tokio tasks
            // and it can be done in parallel.
            let raft_handle = tokio::spawn(serve_raft(addr, raft));
            let heartbeat_handle = tokio::spawn(RaftImpl::heartbeat(
                server_args.raft_addr.to_string(),
                heartbeat_peer_connections,
                heartbeat_raft_state,
                heartbeat_volatile_raft_state,
            ));
            let peer_connections_handle = tokio::spawn(RaftImpl::connect_to_peers(
                server_args.raft_addr,
                server_args.peer_args,
                connect_to_peers_peer_connections,
                connect_to_peers_raft_stable_state,
            ));

            let result = try_join!(
                // Should never return while the server is live
                flatten(raft_handle),
                flatten(heartbeat_handle),
                // Should return relatively quickly after some setup occurs
                flatten(peer_connections_handle),
            );

            match result {
                Ok(_) => Ok(()),
                Err(err) => {
                    log_error(&err);
                    Err(err)
                }
            }
        })
    }
}

async fn serve_raft(addr: SocketAddr, raft: RaftImpl) -> Result<(), StartUpError> {
    Server::builder()
        .add_service(RaftInternalServer::new(raft))
        .serve(addr)
        .await?;

    Ok(())
}

async fn flatten<T, D>(handle: JoinHandle<Result<T, D>>) -> Result<T, StartUpError>
where
    StartUpError: From<D>,
{
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(StartUpError::from(err)),
        Err(_) => Err(StartUpError::CustomError("handling failed".to_owned())),
    }
}

fn log_error(err: &StartUpError) {
    match err {
        StartUpError::FailedToSetup(failures) => {
            tracing::error!("Failed to connect to peers: {:?}", failures);
        }
        StartUpError::FailedToStartServer(err) => {
            tracing::error!("Failed to start the server: {:?}", err);
        }
        StartUpError::FailedToHeartbeat(err) => {
            tracing::error!("Failed to heartbeat: {:?}", err);
        }
        StartUpError::CustomError(err) => {
            tracing::error!("Most likely a concurrency issue: {:?}", err);
        }
        StartUpError::FailedToStartTokioRuntime(err) => {
            tracing::error!("Tokio runtime failed to start: {:?}", err);
        }
    };
}
