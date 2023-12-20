use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::Request;
use tonic::transport::Channel;
use crate::raft_grpc::PingInput;
use crate::raft_grpc::raft_internal_client::RaftInternalClient;

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