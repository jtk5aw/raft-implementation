use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::try_join;
use tokio::time::sleep;
use tonic::transport::{Channel, Server};
use tonic::{Request, Response, Status};

use crate::raft_grpc::raft_internal_client::RaftInternalClient;
use crate::raft_grpc::raft_internal_server::RaftInternalServer;

use self::private::PeerSetup;

use super::raft_grpc::raft_internal_server::RaftInternal;
use super::raft_grpc::{AppendEntriesInput, AppendEntriesOutput};

// Types
type Peers = Arc<Mutex<HashMap<String, Option<RaftInternalClient<Channel>>>>>;

// Errors
pub enum SetupError {
    FailedToConnectToPeers(Vec<String>),
    FailedToStartServer(tonic::transport::Error),
}

impl From<tonic::transport::Error> for SetupError {
    fn from(err: tonic::transport::Error) -> SetupError {
        SetupError::FailedToStartServer(err)
    }
}

// Structs
#[derive(Debug)]
pub struct RaftImpl {
    // Socker Address of the current server
    pub addr: SocketAddr,
    // List of paths to other Raft Nodes
    pub peer_connections: PeerConnections,
}

#[derive(Debug, Clone)]
pub struct PeerConnections {
    pub peers: Peers,
}

// Constructors
impl RaftImpl {
    pub fn new(addr: SocketAddr) -> RaftImpl {
        RaftImpl { 
            addr: addr, 
            peer_connections: PeerConnections {
                peers: Arc::new(Mutex::new(HashMap::new())) 
            }
        }
    }
}

// Private Traits
mod private {
    use std::net::SocketAddr;
    use tonic::transport::Channel;
    use crate::raft_grpc::raft_internal_client::RaftInternalClient;
    use super::SetupError;

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
            &self,
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
}

#[tonic::async_trait] 
impl private::PeerSetup for PeerConnections {
    async fn connect_to_peers(
        &self,
        addr: SocketAddr,
        peers: Vec<String>,
    ) -> Result<(), SetupError> {

        // Create vector of potential failed connections
        let mut error_vec: Vec<String> = Vec::new();

        // Attempt to create all connections
        for peer_port in peers {
            println!("Waiting for peers to start...");
    
            sleep(Duration::from_secs(2)).await;
        
            let result = RaftInternalClient::connect(
                format!("https://[::1]:{}", peer_port)
            ).await;

            match result {
                Ok(client) => self.handle_client_connection(
                    addr, peer_port, client
                ).await,
                Err(err) => {
                    println!("Error connecting to peer: {}", err);
                    error_vec.push(peer_port);
                }
            }
        }

        // Return list of errored connections
        if error_vec.len() > 0 {
            println!("Error vec: {:?}", error_vec);
            Err(SetupError::FailedToConnectToPeers(error_vec))
        } else {
            Ok(())
        }
    }

    async fn handle_client_connection(
        &self,
        addr: SocketAddr,
        peer_port: String,
        mut client: RaftInternalClient<Channel>,
    ) -> () {
        let request = tonic::Request::new(AppendEntriesInput {
            leader_id: addr.port().into(), 
            term: 1,
            entry: vec![peer_port.parse().unwrap()]
        });
    
        let response = client.append_entries(request).await;

        println!("RESPONSE={:?}", response.unwrap().into_inner().success);

        {
            let mut peers = self.peers.lock().unwrap();
            peers.insert(peer_port, Some(client));
        }
    }
}


// Public traits
#[tonic::async_trait]
pub trait RaftImplSetup  {
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
        self
    ) -> Result<(), SetupError>;
}

#[tonic::async_trait]
impl RaftInternal for RaftImpl {
    async fn append_entries(
        &self, 
        request: Request<AppendEntriesInput>,
    ) -> Result<Response<AppendEntriesOutput>, Status> {
        println!("Got a request: {:?}", request);

        let reply = AppendEntriesOutput {
            success: true,
            term: request.into_inner().term,
        };

        Ok(Response::new(reply))
    }
}

#[tonic::async_trait]
impl RaftImplSetup for RaftImpl {
    async fn startup(
        self,
        addr: SocketAddr,
        peers: Vec<String>
    ) -> Result<(), Vec<String>> {

        let peer_connections = self.peer_connections.clone();

        // Starts the server and a "setup" thread to run simultaneously. 
        let result = try_join!(
            self.serve_wrapper(),
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

    async fn serve_wrapper(
        self
    ) -> Result<(), SetupError> {
        // TODO: This feels odd, shouldn't need to do this
        let addr = self.addr.clone();

        let _ = Server::builder()
            .add_service(RaftInternalServer::new(self))
            .serve(addr)
            .await?;

        Ok(())
    }
}


