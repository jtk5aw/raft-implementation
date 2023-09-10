use std::{collections::HashMap, sync::{Mutex, Arc}, net::SocketAddr, time::Duration};

use tokio::time::sleep;
use tonic::{transport::Channel, Request, Response, Status};

use crate::{raft_grpc::{raft_internal_client::RaftInternalClient, raft_internal_server::RaftInternal, AppendEntriesOutput, AppendEntriesInput, GetValueInput, GetValueOutput, ProposeValueOutput, ProposeValueInput, PingInput, PingOutput}, shared::Value};

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
    // List of paths to other Raft Nodes
    pub peer_connections: PeerConnections,
    // Data map TODO: Update this to some more interesting data store (maybe) 
    pub data_store: DataStore,
}

#[derive(Debug, Clone)]
pub struct PeerConnections {
    pub peers: Arc<Mutex<HashMap<String, Option<RaftInternalClient<Channel>>>>>,
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
            data_store : DataStore {
                data: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }
}

#[tonic::async_trait] 
impl PeerSetup for PeerConnections {
    async fn connect_to_peers(
        self,
        addr: SocketAddr,
        peers: Vec<String>,
    ) -> Result<(), SetupError> {

        // Create vector of potential failed connections
        let mut error_vec: Vec<String> = Vec::new();

        println!("Waiting for peers to start...");
    
        sleep(Duration::from_secs(2)).await;

        // Attempt to create all connections
        for peer_port in peers {        
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
        let request = tonic::Request::new(PingInput {
            requester: addr.port().to_string(), 
        });
    
        let response = client.ping(request).await;

        println!("RESPONSE={:?}", response.unwrap());

        {
            let mut peers = self.peers.lock().unwrap();
            peers.insert(peer_port, Some(client));
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
    async fn ping(
        &self,
        request: Request<PingInput>,
    ) -> Result<Response<PingOutput>, Status> {
        println!("Raft got a ping request: {:?}", request);

        let response = PingOutput {
            responder: self.addr.port().to_string(),
        };

        Ok(Response::new(response))
    }

    async fn append_entries(
        &self, 
        request: Request<AppendEntriesInput>,
    ) -> Result<Response<AppendEntriesOutput>, Status> {
        println!("Raft got an append_entries request: {:?}", request);

        let append_entries_input = request.into_inner();

        let values_to_write: Vec<Value> = append_entries_input.entries
            .into_iter()
            .map(|entry| {
                let mut split_iter = entry.split(",");
                (
                    split_iter.next().unwrap().to_owned(), 
                    split_iter.next().unwrap().to_owned(), 
                    split_iter.next().unwrap().to_owned()
                )
            })
            .map(|(_operation, key, value)| Value {
                key: key.to_owned(),
                value: value.to_owned(),
            })
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

    async fn get_value(
        &self,
        request: Request<GetValueInput>,
    ) -> Result<Response<GetValueOutput>, Status> { 
        println!("Raft got a get_value request: {:?}", request);

        let get_value_input = request.into_inner();

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
            values
        };

        Ok(Response::new(reply))
    }

    // TODO: Make this actual Raft
    async fn propose_value(
        &self,
        request: Request<ProposeValueInput>,
    ) -> Result<Response<ProposeValueOutput>, Status> { 
        println!("Raft got a propose_value request: {:?}", request);

        let propose_value_input = request.into_inner();

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
            let log_entries: Vec<String> = propose_value_input.values
                .into_iter()
                .map(|value| {
                    format!("PUT,{},{}", value.key, value.value)
                })
                .collect();

            let peers = self.peer_connections.peers.lock().unwrap().clone();

            futures::future::join_all(peers
                .into_iter()
                .map(|(_key, client)| async {
                    client.unwrap().append_entries(AppendEntriesInput {
                        leader_id: self.addr.port().into(),
                        term: 1,
                        entries: log_entries.to_owned()
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

        let reply = ProposeValueOutput {
            successful: failure_count > 0,
        };

        Ok(Response::new(reply))
    }

}