use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{sleep, self};
use tonic::Request;
use tonic::transport::Channel;
use crate::raft_grpc::PingInput;
use crate::raft_grpc::raft_internal_client::RaftInternalClient;

// Errors
#[derive(Debug)]
pub enum SetupError {
    FailedToConnectToPeers(Vec<String>),
}

// Structs

#[derive(Debug, Clone)]
pub struct PeerConnections {
    pub peers: Arc<Mutex<HashMap<String, PeerData>>>,
}

#[derive(Debug, Clone)]
pub struct PeerData {
    pub connection: Option<RaftInternalClient<Channel>>,
    pub leader_state: Option<RaftLeaderState>,
}

#[derive(Debug, Clone)]
pub struct RaftLeaderState {
    pub next_index: i64,
    pub match_index: i64,
}

// Traits

// TODO: Determine how to prevent "public" from using this
#[tonic::async_trait]
pub trait PeerSetup {
    /**
     * Function used to connect to a set of peers.
     *
     * `Result`
     * `Ok(())` - Connected to all peers
     * `Error(Vec<String>)` - Failed to connect to all peers. Returns list of failures
     */
    async fn connect_to_peers(
        self,
        addr: SocketAddr,
        peers: Vec<String>,
    ) -> Result<(), SetupError>;

    /**
     * Helper function for handling connection setup
     */
    async fn handle_client_connection(
        &self,
        addr: SocketAddr,
        peer_port: String,
        client: RaftInternalClient<Channel>,
    ) -> () ;
}

// Implementations

#[tonic::async_trait]
impl PeerSetup for PeerConnections {
    #[tracing::instrument(skip_all, ret, err(Debug))]
    async fn connect_to_peers(
        self,
        addr: SocketAddr,
        peers: Vec<String>,
    ) -> Result<(), SetupError> {

        // Create vector of potential failed connections
        let mut error_vec: Vec<String> = Vec::new();

        tracing::info!("Waiting for peers to start...");

        sleep(Duration::from_secs(5)).await;

        // Attempt to create all connections
        for peer_port in peers {
            let peer_addr = format!("https://[::1]:{}", peer_port);

            let result = RaftInternalClient::connect(
                peer_addr.to_owned()
            ).await;

            match result {
                Ok(client) => self.handle_client_connection(
                    addr, peer_addr, client
                ).await,
                Err(err) => {
                    tracing::error!(%err, "Error connecting to peer");
                    error_vec.push(peer_port);
                }
            }
        }

        // Return list of errored connections
        if !error_vec.is_empty() {
            tracing::error!(?error_vec, "Error vec");
            Err(SetupError::FailedToConnectToPeers(error_vec))
        } else {
            Ok(())
        }
    }

    #[tracing::instrument(skip_all)]
    async fn handle_client_connection(
        &self,
        addr: SocketAddr,
        peer_addr: String,
        mut client: RaftInternalClient<Channel>,
    ) -> () {
        let request = Request::new(PingInput {
            requester: addr.port().to_string(),
        });

        let response = client.ping(request).await;

        tracing::info!(?response, "RESPONSE");

        {
            let mut peers = self.peers.lock().await;
            peers.insert(
                peer_addr,
                PeerData {
                    connection: Some(client),
                    leader_state: Some(RaftLeaderState {
                        next_index: 1,
                        match_index: 0,
                    }),
                });
        }
    }
}