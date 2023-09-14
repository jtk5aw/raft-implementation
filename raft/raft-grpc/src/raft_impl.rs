use std::{collections::HashMap, sync::{Mutex, Arc}, net::SocketAddr, time::Duration};

use tokio::time::sleep;
use tonic::{transport::Channel, Request, Response, Status};

use crate::{raft_grpc::{raft_internal_client::RaftInternalClient, raft_internal_server::RaftInternal, AppendEntriesOutput, AppendEntriesInput, GetValueInput, GetValueOutput, ProposeValueOutput, ProposeValueInput, PingInput, PingOutput, LogEntry, log_entry::LogAction}, shared::Value};

// Errors
#[derive(Debug)]
pub enum SetupError {
    FailedToConnectToPeers(Vec<String>),
}

// Structs
#[derive(Debug)]
pub struct RaftImpl {
    // Socker Address of the current server
    pub addr: SocketAddr,
    // List of paths to other Raft Nodes. Also contains leader state
    pub peer_connections: PeerConnections,
    // Stable State of the Raft Node (persisted to disk) // TODO: persist this to disk
    pub state: RaftStableState,
    // Volatile State of the Raft Node (not persisted to disk)
    pub volatile_state: RaftVolatileState,
    // Data map TODO: Update this to some more interesting data store (maybe) 
    pub data_store: DataStore,
}

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

#[derive(Debug, Clone)]
pub struct RaftStableState {
    pub raft_data: Arc<Mutex<RaftStableData>>,
}

#[derive(Debug)]
pub struct RaftStableData {
    pub current_term: i64,
    pub voted_for: Option<String>,
    pub log: Vec<LogEntry>,
}

#[derive(Debug, Clone)]
pub struct RaftVolatileState {
    pub raft_data: Arc<Mutex<RaftVolatileData>>,
}

#[derive(Debug)]
pub struct RaftVolatileData {
    pub commit_index: i64,
    pub last_applied: i64,
}

#[derive(Debug, Clone)]
pub struct DataStore {
    pub data: Arc<Mutex<HashMap<String, String>>>,
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

impl RaftImpl {
    /**
     * Creates a new RaftImpl struct
     */
    pub fn new(addr: SocketAddr) -> RaftImpl {
        RaftImpl { 
            addr: addr, 
            peer_connections: PeerConnections {
                peers: Arc::new(Mutex::new(HashMap::new())) 
            },
            state: RaftStableState {
                raft_data: Arc::new(Mutex::new(RaftStableData {
                    current_term: 0,
                    voted_for: None,
                    log: Vec::new(),
                })),
            },
            volatile_state: RaftVolatileState {
                raft_data: Arc::new(Mutex::new(RaftVolatileData {
                    commit_index: 0,
                    last_applied: 0,
                })),
            },
            data_store : DataStore {
                data: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }
}

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
        if error_vec.len() > 0 {
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
        let request = tonic::Request::new(PingInput {
            requester: addr.port().to_string(), 
        });
    
        let response = client.ping(request).await;

        tracing::info!(?response, "RESPONSE");

        {
            let mut peers = self.peers.lock().unwrap();
            peers.insert(
                peer_addr, 
                PeerData {
                    connection: Some(client),
                    leader_state: None,
                });
        }
    }
}

// TODO: ADD AUTH TO THESE CALLS SO THAT IT CAN BE PUBLICLY EXPOSED WITHOUT ISSUE
// Do it using a middleware/extension and probably easiest to just do with an API Key for now. 
// This also  might be an option: https://developer.hashicorp.com/vault/docs/auth/aws
// TODO: Add redirects to the writer. For now assumes that the outside client will only make put requests to the 
// writer. Which make not be true
// TODO: Add suport for other operations when writing to the log. Also actually write to the log
#[tonic::async_trait]
impl RaftInternal for RaftImpl {
    #[tracing::instrument(
        skip(self), 
        name = "Raft:ping",
        ret, 
        err
    )]
    async fn ping(
        &self,
        _request: Request<PingInput>,
    ) -> Result<Response<PingOutput>, Status> {

        let response = PingOutput {
            responder: self.addr.port().to_string(),
        };

        Ok(Response::new(response))
    }

    #[tracing::instrument(
        skip_all,
        name = "Raft:append_entries",
        fields(leader_id, term),
        ret, 
        err
    )]
    async fn append_entries(
        &self, 
        request: Request<AppendEntriesInput>,
    ) -> Result<Response<AppendEntriesOutput>, Status> {

        let append_entries_input = request.into_inner();
        tracing::Span::current().record("leader_id", &append_entries_input.leader_id);
        tracing::Span::current().record("term", &append_entries_input.term);

        let values_to_write: Vec<Value> = append_entries_input.entries
            .into_iter()
            .map(|entry| entry.value.unwrap())
            .collect();

        // Write data locally TODO: Abstract this out into a shared method
        {
            let mut data = self.data_store.data.lock().unwrap();
            values_to_write
                .iter()
                .for_each(|value| {
                    data.insert(value.key.to_owned(), value.value.to_owned());
                });
        }


        let reply = AppendEntriesOutput {
            success: true,
            term: append_entries_input.term,
        };

        Ok(Response::new(reply))
    }

    #[tracing::instrument(
        skip_all,
        name = "Raft:get_value",
        fields(request_id),
        ret, 
        err
    )]
    async fn get_value(
        &self,
        request: Request<GetValueInput>,
    ) -> Result<Response<GetValueOutput>, Status> { 

        let get_value_input = request.into_inner();
        tracing::Span::current().record("request_id", &get_value_input.request_id);

        let values = {
            let data = self.data_store.data.lock().unwrap();

            get_value_input.keys
                .iter()
                .map(|key| {
                    data.get_key_value(key)
                })
                .filter(|key_value_pair| key_value_pair.is_some())
                .map(|key_value_pair| Value {
                    key: key_value_pair.unwrap().0.to_owned(), 
                    value: key_value_pair.unwrap().1.to_owned()
                })
                .collect()
        };

        let reply = GetValueOutput {
            request_id: get_value_input.request_id,
            values
        };

        Ok(Response::new(reply))
    }

    // TODO: Make this actual Raft
    #[tracing::instrument(
        skip_all,
        name = "Raft:propose_value",
        fields(request_id)
        ret, 
        err
    )]
    async fn propose_value(
        &self,
        request: Request<ProposeValueInput>,
    ) -> Result<Response<ProposeValueOutput>, Status> { 

        let propose_value_input = request.into_inner();
        tracing::Span::current().record("request_id", &propose_value_input.request_id);

        // Write data locally TODO: Abstract this out into a shared method
        {
            let mut data = self.data_store.data.lock().unwrap();
            propose_value_input.values
                .iter()
                .for_each(|value| {
                    data.insert(value.key.to_owned(), value.value.to_owned());
                });
        }

        // Communicate new value to peers
        let results = {
            let log_entries: Vec<LogEntry> = propose_value_input.values
                .into_iter()
                .map(|value| {
                    LogEntry {
                        log_action: LogAction::Put.into(),
                        value: Some(value)
                    }
                })
                .collect();

            let peers = self.peer_connections.peers.lock().unwrap().clone();

            futures::future::join_all(peers
                .into_iter()
                .map(|(_key, peer_data)| async {
                    peer_data.connection.unwrap().append_entries(AppendEntriesInput {
                        leader_id: self.addr.to_string(),
                        term: 1,
                        entries: log_entries.to_owned(),
                        // TODO: Implement these to make Raft work
                        prev_log_index: 0,
                        prev_log_term: 0,
                        leader_commit: 0,
                    }).await
                })).await
        }; 

        // Determine how many of peers failed
        let failure_count = results
            .into_iter()
            .map(|result| match result {
                Ok(_) => true,
                Err(_) => false,
            })
            .filter(|success| !success)
            .count();

        tracing::info!(failures=failure_count, "Failure count");

        let reply = ProposeValueOutput {
            request_id: propose_value_input.request_id,
            successful: failure_count == 0, // TODO: Change this to be based on consensus
        };

        Ok(Response::new(reply))
    }

}