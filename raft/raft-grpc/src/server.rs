use std::net::SocketAddr;

use tokio::try_join;

use crate::raft_impl::{SetupError, RaftImpl, PeerSetup, RaftHelper};
 
// Struct for setup
pub struct RisDB {}

// Public traits
#[tonic::async_trait]
pub trait RisDBSetup  {
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
        addr: SocketAddr,
        peers: Vec<String>
    ) -> Result<(), Vec<String>>;
}

#[tonic::async_trait]
impl RisDBSetup for RisDB {
    async fn startup(
        addr: SocketAddr,
        peers: Vec<String>
    ) -> Result<(), Vec<String>> {

        let raft = RaftImpl::new(addr);

        let peer_connections = raft.peer_connections.clone();

        // Starts the server and a "setup" thread to run simultaneously. 
        let result = try_join!(
            raft.serve_wrapper(),
            peer_connections.connect_to_peers(addr, peers)
        );

        if result.is_err() {
            match result.err().unwrap() {
                SetupError::FailedToConnectToPeers(failures) => {
                    println!("Failed to connect to peers: {:?}", failures);
                },
                SetupError::FailedToStartServer(err) => {
                    println!("Failed to start the server: {:?}", err);
                }
            }
        }

        Ok(())
    }
}


