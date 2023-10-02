use std::{collections::HashMap, sync::{Arc}, net::SocketAddr, time::Duration};
use std::cmp::min;
use tokio::sync::Mutex;

use tokio::time::{sleep, timeout};
use tonic::{transport::Channel, Request, Response, Status};

use crate::{raft_grpc::{raft_internal_client::RaftInternalClient, raft_internal_server::RaftInternal, AppendEntriesOutput, AppendEntriesInput, GetValueInput, GetValueOutput, ProposeValueOutput, ProposeValueInput, PingInput, PingOutput, LogEntry, log_entry::LogAction}, shared::Value};

// FUTURE NOTE: The mutex here is the tokio::sync mutex on purpose so that the lock can be held across awaits.
// At the time of writing this was necessary.

// Errors
#[derive(Debug)]
pub enum SetupError {
    FailedToConnectToPeers(Vec<String>),
}

#[derive(Debug)]
pub enum StateMachineError {
    FailedToWriteToLogs(String),
    FailedToApplyLogs(String),
}

#[derive(Debug)]
pub enum LeaderError {
    NoLeaderStateFound(String),
    NoMinimumPeerCommitIndexFound(String),
    NotEnoughPeersUpdated(String),
    FailedToAppendEntries(String),
}


// State Management Structs
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

// Method Return Structs
struct ProposeToPeersResult {
    count_communicated: i64,
    request_append_entries_results: Vec<Result<i64, LeaderError>>
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

    /**
     * Writes
     */
    pub async fn parse_log_entries(
        self,
        input: ProposeValueInput,
    ) -> Result<Vec<LogEntry>, StateMachineError> {
        tracing::info!("Locking state to parse log entries");

        let raft_stable_data = self.state.raft_data.lock().await;

        let log_entries: Vec<LogEntry> = input.values
            .into_iter()
            .map(|value| {
                LogEntry {
                    log_action: LogAction::Put.into(),
                    value: Some(value),
                    term: raft_stable_data.current_term
                }
            })
            .collect();

        tracing::info!("Successfully wrote to logs");

        Ok(log_entries)
    }

    /**
     * Adds the provided LogEntry values to the logs of self and peers.
     *
     * Returns:
     * OK(()): Successfully updated logs for self and a majority of servers overall.
     * Err(String): Failed to update logs of a majority of servers overall.
     */
    pub async fn write_to_logs(
        self,
        log_entries: Vec<LogEntry>
    ) -> Result<(), StateMachineError> {

        tracing::info!("Locking stable and volatile state to add to logs (self and others)");

        let mut raft_stable_data = self.state.raft_data.lock().await;
        let mut raft_volatile_data = self.volatile_state.raft_data.lock().await;

        tracing::info!("Writing new value to own log");

        log_entries
            .iter()
            .for_each(|log_entry| {
                raft_stable_data.log.push(log_entry.to_owned());
            });

        let curr_log_index = log_entries.len() as i64;

        // TODO: Continue retrying to reach peers that failed
        tracing::info!("Proposing new value to peers");

        let propose_to_peers_result = self.propose_new_entries_to_peers(
            curr_log_index,
            raft_volatile_data.commit_index,
            &log_entries
        ).await;

        tracing::info!("Calculate the new commit index");

        match self.calculate_new_commit_index(
            propose_to_peers_result.count_communicated,
            propose_to_peers_result.request_append_entries_results
        ).await {
            Ok(min_new_commit_index) => raft_volatile_data.commit_index = min_new_commit_index,
            Err(_) => tracing::error!("Data not committed not enough peers received updates")
        }

        Ok(())
    }

    /**
     * Makes requests to all peers and tracks how many succeed.
     *
     * ProposeToPeersResult => Object containing both the number of peers that were successfully communicated to
     * and the results of communicating to every peer. Both in successful and unsuccessful cases.
     */
    async fn propose_new_entries_to_peers(
        &self,
        curr_log_index: i64,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>
    ) -> ProposeToPeersResult {
        let mut count_communicated: Arc<Mutex<i64>> = Arc::new(Mutex::new(1)); // Set to 1 to include self
        let request_append_entries_results: Vec<Result<i64, LeaderError>> = {
            futures::future::join_all(self.peer_connections.peers.lock().await
                .iter()
                .map(|(peer_addr, peer_data)| self.request_append_entries_peer(
                    count_communicated.clone(),
                    peer_addr.to_owned(),
                    peer_data.to_owned(),
                    curr_log_index,
                    curr_commit_index,
                    log_entries
                ))
            ).await
        };

        ProposeToPeersResult {
            count_communicated: *count_communicated.clone().lock().await,
            request_append_entries_results
        }
    }

    /**
     * Determine if the number of nodes communicated to is enough to update the commit index.
     * If it is enough, set the new commit index to the minimum among those who have received new data.
     *
     * Ok(i64) => In successful case return what the new minimum commit index is among peers
     * Err(LeaderError) => In failure case, return reason why a new commit index could not be found. e.g
     * Because there was no minimum peer commit index found (even though enough peers received updates) or
     * not enough peers received updates to begin with.
     */
   async fn calculate_new_commit_index(
        &self,
        count_communicated: i64,
        results: Vec<Result<i64, LeaderError>>
    ) -> Result<i64, LeaderError> {
        let peers = self.peer_connections.peers.lock().await;
        if count_communicated >= (peers.len() / 2) as i64 {
            Ok(
                results
                    .into_iter()
                    .filter_map(|result| result.ok())
                    .min()
                    .ok_or_else(|| LeaderError::NoMinimumPeerCommitIndexFound(
                        "Failed to get minimum peer commit index after successful commincation to the majority".to_owned()
                    ))?
            )
        } else {
            Err(LeaderError::NotEnoughPeersUpdated(
                "Not enough peers were updated to get a new commit index".to_owned()
            ))
        }
    }

    /**
     * Make a request to a peer to append entries. 
     * 
     * Returns: 
     * OK(()): Successfully appended entries
     * Err(String): Failed to append entries for the provided address (the address is the key of the peer)
     */
    async fn request_append_entries_peer(
        &self,
        count_communicated: Arc<Mutex<i64>>,
        peer_addr: String,
        peer_data: PeerData,
        curr_log_index: i64,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>,
    ) -> Result<i64, LeaderError>{
        let mut peer_leader_state = peer_data.leader_state
            .ok_or_else(|| LeaderError::NoLeaderStateFound("No leader state found".to_owned()))?;
        
        // Make request to append entries TODO: Play with the timeout time here
        match timeout(Duration::from_secs(5), peer_data.connection.unwrap().append_entries(AppendEntriesInput {
            leader_id: self.addr.to_string(),
            term: 0, // TODO: Actually handle different terms
            entries: log_entries.to_owned(),
            prev_log_index: (curr_log_index - 1),
            prev_log_term: 0, // TODO: Actually handle different terms
            leader_commit: curr_commit_index,
        })).await {
            Ok(_) => {
                tracing::info!(?peer_addr, "Successfully appended entries to peer");
                // TODO; Handle getting a different term in response and also different index (i.e its behind). May need to make re requests in some cases
                peer_leader_state.next_index = curr_log_index + 1;
                peer_leader_state.match_index = curr_log_index;

                if peer_leader_state.match_index >= curr_log_index {
                    tracing::info!("Peer now has a match index where all new logs are applied. Updated count of peers communicated to");
                    *count_communicated.lock().await += 1;
                }

                Ok(peer_leader_state.match_index)
            },
            Err(e) => {
                tracing::error!(?e, ?peer_addr, "Failed to append entries to peer");
                Err(LeaderError::FailedToAppendEntries(peer_addr.to_owned()))
            }
        }
    }

    /**
     * Check if last_applied < commit_index. As long as that statement is true, the log
     * at last_applied + 1 will be applied to the state machine. 
     * 
     * Returns: 
     * Ok(()): All logs applied successfully and last_applied = commit_index now. 
     * Err(_): Some issue while attempting to apply logs to the state machine. last_applied is one less than 
     *         the index that causes the failure to occur. I.E if index 2 doesn't contain a value and can't be written
     *         to the state machine, last_applied will be 1 when the error is returned.
     */
    pub async fn update_state_machine(
        self, 
    ) -> Result<(), StateMachineError> {

        let mut volatile_state = self.volatile_state.raft_data.lock().await;

        while volatile_state.last_applied < volatile_state.commit_index {
            let raft_state = self.state.raft_data.lock().await;

            let index_to_apply = volatile_state.last_applied + 1;
            let log_entry_to_apply = &raft_state.log[index_to_apply as usize];

            self.apply_to_state_machine(log_entry_to_apply).await?;

            volatile_state.last_applied += 1;
        }

        Ok(())
    }

    /**
     * Takes a single log entry and applies it to the state machine. 
     */
    async fn apply_to_state_machine(
        &self,
        log_entry_to_apply: &LogEntry,
    ) -> Result<(), StateMachineError> {
        let mut data = self.data_store.data.lock().await;

        match LogAction::from_i32(log_entry_to_apply.log_action) {
            Some(LogAction::Put) => {
                let value = log_entry_to_apply.value
                    .as_ref()
                    .ok_or_else(|| StateMachineError::FailedToApplyLogs("Failed to get log entries value".to_owned()))?;
                
                data.insert(
                    value.key.to_owned(), 
                    value.value.to_owned()
                );
    
                Ok(())
            },
            Some(LogAction::Delete) => {
                todo!("Delete functionality is not yet implemented")
            },
            None => {
                Err(StateMachineError::FailedToApplyLogs("No log action was stored".to_owned()))
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

// TODO: ADD AUTH TO THESE CALLS SO THAT IT CAN BE PUBLICLY EXPOSED WITHOUT ISSUE
// Do it using a middleware/extension and probably easiest to just do with an API Key for now. 
// This also  might be an option: https://developer.hashicorp.com/vault/docs/auth/aws
// TODO: Add redirects to the writer. For now assumes that the outside client will only make put requests to the 
// writer. Which make not be true
// TODO: Add suport for other operations when writing to the log.
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

        tracing::info!("Locking state to append entries");

        {
            let mut raft_stable_data = self.state.raft_data.lock().await;
            let mut raft_volatile_data = self.volatile_state.raft_data.lock().await;

            tracing::info!("Parsing the provided  input into log")


            tracing::info!("Checking if provided term is accepted");

            let response = if append_entries_input.term < raft_stable_data.current_term {
                tracing::warn!("Term provided has already been surpassed. Return failure and current term");

                AppendEntriesOutput {
                    success: false,
                    term: raft_stable_data.current_term
                }
            } else if raft_stable_data.log
                .get(append_entries_input.prev_log_index)
                .map_or_else(
                    |val| append_entries_input.prev_log_term != val.term,
                    || false
                ) {
                tracing::info!("Term of previous log index does not match or does not exist. Return failure and highest prev log term");

                AppendEntriesOutput {
                    success: false,
                    term: min((raft_stable_data.log.len() - 1) as i64, 0)
                }
            } else {
                tracing::info!("Clear the log from the provided index on of any entries not matching the provided term");

                raft_stable_data.log.truncate((append_entries_input.prev_log_index + 1) as usize);

                tracing::info!("Appending new entries to the log");

                self.write_to_logs(append_entries_input.entries).await?;

                // TODO TODO TODO: Start from figuring out what should happen after writing to logs here
                //  TODO TODO TODO

            };
        }

        let values_to_write: Vec<Value> = append_entries_input.entries
            .into_iter()
            .map(|entry| entry.value.unwrap())
            .collect();

        // Write data locally TODO: Abstract this out into a shared method
        {
            let mut data = self.data_store.data.lock().await;
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
            let data = self.data_store.data.lock().await;

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

    // TODO: Return a status error when maybe it makes more sense to return Ok with success: false?
    //  or maybe consider removing that boolean and just return certain status failures in success: false case instead
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
        let request_id = propose_value_input.request_id.clone();
        tracing::Span::current().record("request_id", &request_id);

        tracing::info!("Converting input data to log entries");

        let log_entries = self.parse_log_entries(propose_value_input).await
            .map_err(|err| {
                tracing::error!(err = ?err, "Failed to properly parse logs");
                Status::internal("Failed to parse provided input".to_owned())
            })?;

        tracing::info!("Add new values to own logs and to peer logs");

        match self.write_to_logs(log_entries).await {
            Ok(_) => {
                tracing::info!("Successfully communicated value to majority of peers");
                Ok(())
            },
            Err(err) => {
                tracing::error!(err = ?err, "Failed to communicate value to majority of peers");
                Err(Status::internal("Failed to write provided values".to_owned()))
            }
        }?;

        tracing::info!("Applying any necessary changes to the state machine");

        match self.update_state_machine().await {
          Ok(_) => {
              tracing::info!("Successful updated the state maching");
              Ok(())
          },
            Err(err) => {
                tracing::error!(err = ?err, "Failed to update the state matching");
                Err(Status::internal("Failed to apply provided values".to_owned()))
            }
        }?;

        tracing::info!("Returning result to client");

        let reply = ProposeValueOutput {
            request_id,
            successful: true
        };

        Ok(Response::new(reply))
    }

}