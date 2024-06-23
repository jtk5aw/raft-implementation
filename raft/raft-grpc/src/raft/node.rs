use crate::database::PeerArgs;
use crate::raft::data_store::{DataStore, StateMachine};
use crate::raft::peer::{Candidate, Leader, PeerConnections, PeerSetup, StateMachineError};
use crate::raft::state::{
    RaftNodeType, RaftStableData, RaftStableState, RaftVolatileData, RaftVolatileState,
};
use crate::shared::Value;
use crate::structs::log_entry::LogAction;
use crate::structs::raft_internal_server::RaftInternal;
use crate::structs::{
    AppendEntriesInput, AppendEntriesOutput, GetValueInput, GetValueOutput, LogEntry, PingInput,
    PingOutput, ProposeValueInput, ProposeValueOutput, RequestVoteInput, RequestVoteOutput,
};
use rand::Rng;
use std::cmp::{min, Ordering};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{interval, sleep, MissedTickBehavior};
use tonic::{Request, Response, Status};

// Constants
const HEARTBEAT_INTERVAL: u64 = 500;

// Errors

#[derive(Debug)]
pub enum HeartbeatError {
    FailedToPingPeers(StateMachineError),
    CustomError(String),
}

impl From<StateMachineError> for HeartbeatError {
    fn from(err: StateMachineError) -> HeartbeatError {
        HeartbeatError::FailedToPingPeers(err)
    }
}

impl From<String> for HeartbeatError {
    fn from(err: String) -> HeartbeatError {
        HeartbeatError::CustomError(err)
    }
}

#[derive(Debug)]
pub enum SetupError {
    FailedToConnectToPeers(Vec<String>),
}

#[derive(Debug)]
pub enum ReadError {}
#[derive(Debug)]
pub enum WriteError {
    NotTheLeader,
    FailedToParseLogs(StateMachineError),
    FailedToCommunicateToMajority(StateMachineError),
    FailedToUpdateStateMachine(StateMachineError),
}

// TODO: Take stock of all the Arcs I'm using and see if they can be compressed
// and some can be removed.

#[derive(Debug)]
pub struct Database {
    /// Socket Address of the current server
    pub addr: SocketAddr,
    /// List of paths to other Raft Nodes. Also contains leader state
    pub(crate) peer_connections: PeerConnections,
    /// Stable State of the Raft Node (persisted to disk) // TODO: persist this to disk
    pub(crate) state: RaftStableState,
    /// Volatile State of the Raft Node (not persisted to disk)
    pub(crate) volatile_state: RaftVolatileState,
    /// Data map TODO: Update this to some more interesting data store (maybe)
    pub(crate) data_store: DataStore,
}

#[tonic::async_trait]
pub trait ReadAndWrite {
    /// Takes the provided `GetValueInput` and uses it to construct a `GetValueOutput`
    /// object
    ///
    /// get_value_input: Contains the set of keys to retrieve value for
    ///
    /// WARNING: As of right now no `ReadError` will ever be returned. If I want to update this in
    /// the future to be more interesting it will need to be able to return errors so I'm adding it to
    /// the api now.
    ///
    /// Returns:
    /// Ok(GetValueOutput): `Value`s corresponding to the provided keys if they exist
    /// Err(ReadError): `ReadError` representing the issue encountered while trying to
    ///                  retrieve the provided keys.
    async fn get_value(&self, get_value_input: GetValueInput) -> Result<GetValueOutput, ReadError>;

    /// Takes a provided `ProposeValueInput` and attempts to append the provided
    /// `Value`s to the nodes Log while also sharing with a majority of peers. Assuming
    /// a majority of peers receive and acknowledge apply the value as well, a success will
    /// be returned and the value will be inserted into the data store.
    ///
    /// Returns:
    /// Ok(ProposeValueOutput): `Value`s are properly inserted to the database after sharing with peers
    /// Err(WriteError): `Value`s were not properly inserted.
    ///
    /// propse_value_input: Contains the set of `Value`s to insert.
    async fn propose_value(
        &self,
        propose_value_input: ProposeValueInput,
    ) -> Result<ProposeValueOutput, WriteError>;
}

#[tonic::async_trait]
trait Follower {
    /**
     * Takes the value provided from a peer and applies it to the log.
     * Before doing so it truncates the log from the provided Previous Log Index + 1.
     * This will also update the state machine if the commit index has increased.
     *
     * Returns:
     * Ok(AppendEntriesOutput): Output determine if update was successful.
     * Err(StateMachineError): The error faced when attempting to modify the state machine.
     */
    async fn make_update_from_peer(
        &self,
        raft_stable_data: &mut RaftStableData,
        raft_volatile_data: &mut RaftVolatileData,
        append_entries_input: AppendEntriesInput,
    ) -> Result<AppendEntriesOutput, StateMachineError>;

    /**
     * Adds the provided LogEntry values to own servers logs.
     */
    async fn write_to_logs(&self, stable_data: &mut RaftStableData, log_entries: Vec<LogEntry>);
}

#[derive(Debug)]
pub struct RaftImpl {
    /// Representation of the database that this node is responsible for maintaining
    pub inner: Arc<Database>,
}
impl RaftImpl {
    /**
     * Creates a new RaftImpl struct
     */
    pub fn new(inner: Arc<Database>) -> RaftImpl {
        RaftImpl { inner }
    }

    /// Function used to heartbeat on this node. Behavior of the heartbeat is
    /// different depending on the RaftNodeType of th enode.
    ///
    /// `Result`
    ///`Ok(())` - Stopped heartbeating
    /// `Error(HeartbeatError)` - Failed to complete heartbeat for some reason.
    ///
    // TODO: This heartbeat thread leads to one massing and basically impossible
    //  to debug span. If want to set up otel or something like it this should be tweaked
    //  to make it's own spans on every "tick"
    #[tracing::instrument(skip_all, ret, err(Debug))]
    pub async fn heartbeat(
        addr: String,
        peer_connections: PeerConnections,
        raft_stable_state: RaftStableState,
        raft_volatile_state: RaftVolatileState,
    ) -> Result<(), HeartbeatError> {
        // Won't do a bunch of ticks at once to "catch up"
        let mut interval = interval(Duration::from_millis(HEARTBEAT_INTERVAL));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            let node_type = { raft_stable_state.raft_data.lock().await.node_type.clone() };
            match node_type {
                RaftNodeType::Follower(_) | RaftNodeType::Candidate => {
                    // Wait a little longer for followers and candidates
                    let random = {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(50..150)
                    };
                    sleep(Duration::from_millis(random)).await;
                }
                _ => {
                    // do nothing
                }
            }
            let mut raft_stable_data = raft_stable_state.raft_data.lock().await;

            match raft_stable_data.node_type {
                RaftNodeType::StartingUp => {
                    tracing::info!("Not all peers connected");
                    sleep(Duration::from_secs(5)).await;
                }
                RaftNodeType::Follower(pinged) => {
                    if pinged {
                        tracing::info!("Already pinged, reset follower status");
                        raft_stable_data.node_type = RaftNodeType::Follower(false);
                        continue;
                    }

                    tracing::info!("Convert to candidate");
                    raft_stable_data.node_type = RaftNodeType::Candidate;
                    raft_stable_data.voted_for = Some(addr.to_owned());
                    raft_stable_data.current_term += 1;
                    tracing::info!(raft_stable_data.current_term, "Begin request for votes");

                    let voted_leader = peer_connections
                        .request_votes(addr.to_owned(), &raft_stable_data)
                        .await
                        .won_election;

                    if voted_leader {
                        tracing::info!("Successfully voted to be leader");
                        raft_stable_data.node_type = RaftNodeType::Leader;
                    } else {
                        tracing::info!(
                            "Not voted leader. Remain candidate unless leader heartbeat received"
                        );
                        raft_stable_data.voted_for = None;
                    }
                }
                RaftNodeType::Leader => {
                    tracing::info!("Leader sharing heartbeat log");
                    let mut raft_volatile_data = raft_volatile_state.raft_data.lock().await;
                    let log_entries = vec![LogEntry {
                        log_action: LogAction::Noop.into(),
                        value: None,
                        term: raft_stable_data.current_term,
                    }];

                    match peer_connections
                        .write_and_share_logs(
                            addr.to_string(),
                            raft_stable_data.deref_mut(),
                            raft_volatile_data.deref_mut(),
                            log_entries,
                        )
                        .await
                    {
                        Ok(_) => {
                            tracing::info!("Successfully communicated value to majority of peers");
                            Ok(())
                        }
                        Err(err) => {
                            tracing::error!(err = ?err, "Failed to communicate value to majority of peers");
                            Err(err)
                        }
                    }?;
                }
                RaftNodeType::Candidate => {
                    tracing::info!("Already candidate, ask for votes again");
                    raft_stable_data.voted_for = Some(addr.to_owned());
                    raft_stable_data.current_term += 1;
                    tracing::info!(raft_stable_data.current_term, "Beging request for votes");

                    let voted_leader = peer_connections
                        .request_votes(addr.to_owned(), &raft_stable_data)
                        .await
                        .won_election;

                    if voted_leader {
                        tracing::info!("Successfully voted for leader");
                        raft_stable_data.node_type = RaftNodeType::Leader;
                    } else {
                        tracing::info!(
                            "Not voted leader. Remain candidate unless leader heartbeat received"
                        );
                        raft_stable_data.voted_for = None;
                    }
                }
            }
        }
    }

    /**
     * Function used to connect to a set of peers.
     *
     * `Result`
     * `Ok(())` - Connected to all peers
     * `Error(Vec<String>)` - Failed to connect to all peers. Returns list of failures
     */
    #[tracing::instrument(skip_all, ret, err(Debug))]
    pub async fn connect_to_peers(
        addr: SocketAddr,
        peers: Vec<PeerArgs>,
        peer_connections: PeerConnections,
        raft_stable_state: RaftStableState,
    ) -> Result<(), SetupError> {
        // Create vector of potential failed connections
        let mut error_vec: Vec<String> = Vec::new();

        tracing::info!("Waiting for peers to start...");

        sleep(Duration::from_secs(5)).await;

        // Attempt to create all connections
        for peer in peers {
            let peer_addr = peer.addr;
            let mut attempt = 0;

            while attempt < 10 {
                match peer_connections
                    .handle_client_connection(addr, peer_addr.to_string())
                    .await
                {
                    Ok(_) => {
                        tracing::info!("Successfully connected to peer");
                        break;
                    }
                    Err(err) => {
                        tracing::warn!(peer_addr, attempt, "Failed to connect: {:?}", err);
                        attempt += 1;
                        tracing::warn!("Sleep for 5 seconds ...");
                        sleep(Duration::from_secs(5)).await;
                    }
                };
            }

            if attempt == 10 {
                error_vec.push(peer_addr.to_string());
            }
        }

        // Return list of errored connections
        if !error_vec.is_empty() {
            tracing::error!(?error_vec, "Error vec");
            Err(SetupError::FailedToConnectToPeers(error_vec))
        } else {
            // Convert to follower
            let mut raft_stable_data = raft_stable_state.raft_data.lock().await;
            raft_stable_data.node_type = RaftNodeType::Follower(false);

            Ok(())
        }
    }
}

// TODO: ADD AUTH TO THESE CALLS SO THAT IT CAN BE PUBLICLY EXPOSED WITHOUT ISSUE
// Do it using a middleware/extension and probably easiest to just do with an API Key for now.
// This also  might be an option: https://developer.hashicorp.com/vault/docs/auth/aws
// TODO: Add redirects to the writer. For now assumes that the outside client will only make put requests to the
// writer. Which make not be true
#[tonic::async_trait]
impl RaftInternal for RaftImpl {
    // TODO: Return a status error when maybe it makes more sense to return Ok with success: false?
    //  or maybe consider removing that boolean and just return certain status failures in success: false case instead
    #[allow(useless_deprecated)]
    #[deprecated]
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

        let _ = self
            .inner
            .propose_value(propose_value_input)
            .await
            .map_err(|err| {
                tracing::error!(?err, "Failed to propose the new value");
                Status::internal(format!("Failed to write the new values due to: {:?}", err))
            })?;

        let reply = ProposeValueOutput { request_id };

        Ok(Response::new(reply))
    }

    #[allow(useless_deprecated)]
    #[deprecated]
    #[tracing::instrument(skip_all, name = "Raft:get_value", fields(request_id), ret, err)]
    async fn get_value(
        &self,
        request: Request<GetValueInput>,
    ) -> Result<Response<GetValueOutput>, Status> {
        let get_value_input = request.into_inner();
        tracing::Span::current().record("request_id", &get_value_input.request_id);

        let copied_request_id = get_value_input.request_id.to_owned();

        let output = self.inner.get_value(get_value_input).await.map_err(|err| {
            Status::internal(format!(
                "Failed to get value from the state machine: {:?}",
                err
            ))
        })?;

        let reply = GetValueOutput {
            request_id: copied_request_id,
            values: output.values,
        };

        Ok(Response::new(reply))
    }

    #[tracing::instrument(
        skip_all,
        name = "Raft:request_vote",
        fields(candidate_id, requested_term),
        ret,
        err
    )]
    async fn request_vote(
        &self,
        request: Request<RequestVoteInput>,
    ) -> Result<Response<RequestVoteOutput>, Status> {
        let request_vote_input = request.into_inner();
        tracing::Span::current().record("candidate_id", &request_vote_input.candidate_id);
        tracing::Span::current().record("requested_term", &request_vote_input.term);

        let raft_stable_state = self.inner.state.raft_data.lock().await;

        // Candidate is requesting votes on an old term. Vote no
        if raft_stable_state.current_term > request_vote_input.term {
            tracing::info!("Candidate is requesting votes on an old term. Vote no");
            return Ok(Response::new(RequestVoteOutput {
                term: raft_stable_state.current_term,
                vote_granted: false,
            }));
        }

        // Already voted for another candidate. Vote no
        if raft_stable_state
            .voted_for
            .as_ref()
            .is_some_and(|voted| voted != &request_vote_input.candidate_id)
        {
            tracing::info!("Already voted for another candidate. Vote no");
            return Ok(Response::new(RequestVoteOutput {
                term: raft_stable_state.current_term,
                vote_granted: false,
            }));
        }

        // Candidate has out of date logs. Vote no
        if !candidate_more_up_to_date(&raft_stable_state, &request_vote_input) {
            tracing::info!("Candidate has out of date logs. Vote no");
            return Ok(Response::new(RequestVoteOutput {
                term: raft_stable_state.current_term,
                vote_granted: false,
            }));
        }

        // Candidate has a recent enough term and is more up to date than the current node. Vote yes
        let mut raft_stable_state = raft_stable_state;
        raft_stable_state.voted_for = Some(request_vote_input.candidate_id);
        raft_stable_state.current_term = request_vote_input.term;
        raft_stable_state.node_type = RaftNodeType::Follower(true);

        Ok(Response::new(RequestVoteOutput {
            term: request_vote_input.term, // TODO: Determine if this is the term that should be returned in the case where vote is granted
            vote_granted: true,
        }))
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
        tracing::Span::current().record("term", append_entries_input.term);

        tracing::info!("Locking state to append entries");

        let mut raft_stable_data = self.inner.state.raft_data.lock().await;
        let mut raft_volatile_data = self.inner.volatile_state.raft_data.lock().await;

        tracing::info!("Checking if provided term is accepted");

        if append_entries_input.term < raft_stable_data.current_term {
            tracing::warn!(
                "Term provided has already been surpassed. Return failure and current term"
            );

            return Ok(Response::new(AppendEntriesOutput {
                success: false,
                term: raft_stable_data.current_term,
            }));
        };

        // As a base case, if the previous term is the first log entry is matches
        let prev_term_matches = append_entries_input.prev_log_index == 0
            || raft_stable_data
                .log
                .get(append_entries_input.prev_log_index as usize)
                .map_or_else(
                    || false,
                    |val| {
                        tracing::info!(
                            append_entries_input.prev_log_term,
                            val.term,
                            "Comparing input prev_log_term with logs term"
                        );
                        append_entries_input.prev_log_term == val.term
                    },
                );

        if !prev_term_matches {
            tracing::info!("Term of previous log index does not match or does not exist. Return failure and prev log term");

            return Ok(Response::new(AppendEntriesOutput {
                success: false,
                term: raft_stable_data.log[raft_stable_data.log.len() - 1].term,
            }));
        };

        tracing::info!("Marking self as pinged");
        raft_stable_data.node_type = RaftNodeType::Follower(true);

        tracing::info!("Updating Logs");
        let output = self
            .inner
            .make_update_from_peer(
                raft_stable_data.deref_mut(),
                raft_volatile_data.deref_mut(),
                append_entries_input,
            )
            .await
            .map_err(|_| Status::internal("Failed to update the state machine"))?;

        return Ok(Response::new(output));
    }

    #[tracing::instrument(skip(self), name = "Raft:ping", ret, err)]
    async fn ping(&self, _request: Request<PingInput>) -> Result<Response<PingOutput>, Status> {
        let response = PingOutput {
            responder: self.inner.addr.port().to_string(),
        };

        Ok(Response::new(response))
    }
}

impl Database {
    pub fn new(addr: SocketAddr) -> Self {
        Database {
            addr,
            peer_connections: PeerConnections {
                peers: Arc::new(Mutex::new(HashMap::new())),
            },
            state: RaftStableState {
                raft_data: Arc::new(Mutex::new(RaftStableData {
                    node_type: RaftNodeType::StartingUp,
                    current_term: 0,
                    voted_for: None,
                    // Add a dummy value to the start of the log that should NEVER actually be applied. But starting from
                    // 0 causes problems cause last applied and commit index also start at 0
                    // TODO: Modify the type of this so you can literally only append to it. Make it so
                    // this first entry can never be overwritten
                    log: vec![LogEntry {
                        log_action: LogAction::Noop.into(),
                        value: None,
                        term: -1,
                    }],
                })),
            },
            volatile_state: RaftVolatileState {
                raft_data: Arc::new(Mutex::new(RaftVolatileData {
                    commit_index: 0,
                    last_applied: 0,
                })),
            },
            data_store: DataStore {
                data: Arc::new(Mutex::new(HashMap::new())),
            },
        }
    }
}

#[tonic::async_trait]
impl Follower for Database {
    #[tracing::instrument(skip_all, ret, err(Debug))]
    async fn make_update_from_peer(
        &self,
        raft_stable_data: &mut RaftStableData,
        raft_volatile_data: &mut RaftVolatileData,
        append_entries_input: AppendEntriesInput,
    ) -> Result<AppendEntriesOutput, StateMachineError> {
        tracing::info!("Clear the log from the provided index on of any entries not matching the provided term");

        raft_stable_data
            .log
            .truncate((append_entries_input.prev_log_index + 1) as usize);

        tracing::info!("Appending new entries to the log");

        self.write_to_logs(raft_stable_data, append_entries_input.entries)
            .await;

        if raft_volatile_data.commit_index < append_entries_input.leader_commit {
            tracing::info!("Updating commit index");
            raft_volatile_data.commit_index = min(
                (raft_stable_data.log.len() - 1) as i64,
                append_entries_input.leader_commit,
            );
        }

        tracing::info!("Attempting to update state machine");

        match self
            .data_store
            .update_state_machine(raft_stable_data, raft_volatile_data)
            .await
        {
            Ok(num_updates) => {
                tracing::info!(?num_updates, "Successful updated the state machine");
                Ok(AppendEntriesOutput {
                    success: true,
                    term: 0, // TODO: Make this handle term properly and confirm other aspects of this method handle term correctly
                })
            }
            Err(err) => {
                tracing::error!(err = ?err, "Failed to update the state machine");
                Err(err)
            }
        }
    }

    async fn write_to_logs(&self, stable_data: &mut RaftStableData, log_entries: Vec<LogEntry>) {
        tracing::info!("Writing new value to own log");

        log_entries.iter().for_each(|log_entry| {
            stable_data.log.push(log_entry.to_owned());
        });
    }
}

#[tonic::async_trait]
impl ReadAndWrite for Database {
    #[tracing::instrument(skip_all, ret, err(Debug))]
    async fn get_value(&self, get_value_input: GetValueInput) -> Result<GetValueOutput, ReadError> {
        let data = self.data_store.data.lock().await;

        let values = get_value_input
            .keys
            .iter()
            .map(|key| data.get_key_value(key))
            .filter(|key_value_pair| key_value_pair.is_some())
            .map(|key_value_pair| Value {
                key: key_value_pair.unwrap().0.to_owned(),
                value: key_value_pair.unwrap().1.to_owned(),
            })
            .collect();

        Ok(GetValueOutput {
            request_id: get_value_input.request_id.clone(),
            values,
        })
    }

    #[tracing::instrument(skip_all, ret, err(Debug))]
    async fn propose_value(
        &self,
        propose_value_input: ProposeValueInput,
    ) -> Result<ProposeValueOutput, WriteError> {
        tracing::debug!("Locking state");

        let request_id = propose_value_input.request_id.clone();
        let mut raft_stable_data = self.state.raft_data.lock().await;
        let mut raft_volatile_data = self.volatile_state.raft_data.lock().await;

        tracing::debug!("Checking if node is leader");

        match raft_stable_data.node_type {
            RaftNodeType::Leader => {
                tracing::debug!("Node is leader, continue");
            }
            _ => {
                tracing::error!("Node is not the leader cancel request");
                return Err(WriteError::NotTheLeader);
            }
        }

        tracing::debug!("Converting input data to log entries");

        let log_entries = parse_log_entries(raft_stable_data.deref(), propose_value_input)
            .await
            .map_err(|err| {
                tracing::error!(err = ?err, "Failed to properly parse logs");
                WriteError::FailedToParseLogs(err)
            })?;

        tracing::debug!("Add new values to own logs and to peer logs");

        match self
            .peer_connections
            .write_and_share_logs(
                self.addr.to_string(),
                raft_stable_data.deref_mut(),
                raft_volatile_data.deref_mut(),
                log_entries,
            )
            .await
        {
            Ok(_) => {
                tracing::debug!("Successfully communicated value to majority of peers");
                Ok(())
            }
            Err(err) => {
                tracing::error!(err = ?err, "Failed to communicate value to majority of peers");
                Err(WriteError::FailedToCommunicateToMajority(err))
            }
        }?;

        tracing::debug!("Applying any necessary changes to the state machine");

        match self
            .data_store
            .update_state_machine(raft_stable_data.deref(), raft_volatile_data.deref_mut())
            .await
        {
            Ok(num_updates) => {
                tracing::debug!(?num_updates, "Successful updated the state machine");
                Ok(())
            }
            Err(err) => {
                tracing::error!(err = ?err, "Failed to update the state machine");
                Err(WriteError::FailedToUpdateStateMachine(err))
            }
        }?;

        tracing::debug!("Returning result to client");

        Ok(ProposeValueOutput { request_id })
    }
}

// Helper functions
async fn parse_log_entries(
    raft_stable_data: &RaftStableData,
    input: ProposeValueInput,
) -> Result<Vec<LogEntry>, StateMachineError> {
    let log_entries: Vec<LogEntry> = input
        .values
        .into_iter()
        .map(|value| LogEntry {
            log_action: LogAction::Put.into(),
            value: Some(value),
            term: raft_stable_data.current_term,
        })
        .collect();

    tracing::info!("Successfully wrote to logs");

    Ok(log_entries)
}

/**
 * Takes the current raft stable state and a request vote input and determines if
 * the request or current node have more up to date logs.
 *
 * Taken directly from the Raft paper
 * > "Raft determines which of two logs is more up-to-date
 * by comparing the index and term of the last entries in the
 * logs. If the logs have last entries with different terms, then
 * the log with the later term is more up-to-date. If the logs
 * end with the same term, then whichever log is longer is
 * more up-to-date"
 *
 * Result:
 * true -> Peer is more up to date
 * false -> Current node is more up to date
 */
fn candidate_more_up_to_date(raft_stable_state: &RaftStableData, input: &RequestVoteInput) -> bool {
    // WARNING: Right now there is logic to always inject at least one Noop log entry to the log in the beginning
    // if this changes this will cause panics
    let last_log_term = raft_stable_state.log.last().unwrap().term;
    let candidate_last_log_term = input.last_log_term;

    match candidate_last_log_term.cmp(&last_log_term) {
        Ordering::Less => false,
        Ordering::Greater => true,
        Ordering::Equal => {
            let last_log_index = (raft_stable_state.log.len() - 1) as i64;
            let candidate_last_log_index = input.last_log_index;

            tracing::info!(
                ?last_log_index,
                ?candidate_last_log_index,
                "Comparing log indexes"
            );

            if candidate_last_log_index >= last_log_index {
                return true;
            }

            return false;
        }
    }
}
