use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};
use crate::raft::data_store::{DataStore, StateMachine};
use crate::raft::follower::FollowerActions;
use crate::raft::leader::LeaderActions;
use crate::raft::peer::PeerConnections;
use crate::raft::state::{RaftStableData, RaftStableState, RaftVolatileData, RaftVolatileState};
use crate::raft_grpc::{AppendEntriesInput, AppendEntriesOutput, GetValueInput, GetValueOutput, LogEntry, PingInput, PingOutput, ProposeValueInput, ProposeValueOutput};
use crate::raft_grpc::log_entry::LogAction;
use crate::raft_grpc::raft_internal_server::RaftInternal;
use crate::shared::Value;

// Errors TODO: Try to make it so this doesn't have to be shared at the top level
#[derive(Debug)]
pub enum StateMachineError {
    FailedToWriteToLogs(String),
    FailedToApplyLogs(String),
}

#[derive(Debug)]
pub struct RaftImpl {
    // Socket Address of the current server
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

impl RaftImpl {
    /**
     * Creates a new RaftImpl struct
     */
    pub fn new(addr: SocketAddr) -> RaftImpl {
        RaftImpl {
            addr,
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
            data_store: DataStore {
                data: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }
}

// TODO: ADD AUTH TO THESE CALLS SO THAT IT CAN BE PUBLICLY EXPOSED WITHOUT ISSUE
// Do it using a middleware/extension and probably easiest to just do with an API Key for now.
// This also  might be an option: https://developer.hashicorp.com/vault/docs/auth/aws
// TODO: Add redirects to the writer. For now assumes that the outside client will only make put requests to the
// writer. Which make not be true
// TODO: Add support for other operations when writing to the log.
#[tonic::async_trait]
impl RaftInternal for RaftImpl {
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

        tracing::info!("Locking state");

        let mut raft_stable_data = self.state.raft_data.lock().await;
        let mut raft_volatile_data = self.volatile_state.raft_data.lock().await;

        tracing::info!("Converting input data to log entries");

        let log_entries = parse_log_entries(
            raft_stable_data.deref(),
            propose_value_input
        ).await
            .map_err(|err| {
                tracing::error!(err = ?err, "Failed to properly parse logs");
                Status::internal("Failed to parse provided input".to_owned())
            })?;

        tracing::info!("Add new values to own logs and to peer logs");

        match self.write_and_share_logs(
            raft_stable_data.deref_mut(),
            raft_volatile_data.deref_mut(),
            log_entries
        ).await {
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

        match self.update_state_machine(
            raft_stable_data.deref(),
            raft_volatile_data.deref_mut()
        ).await {
            Ok(num_updates) => {
                tracing::info!(?num_updates, "Successful updated the state matchine");
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

        let mut raft_stable_data = self.state.raft_data.lock().await;
        let mut raft_volatile_data = self.volatile_state.raft_data.lock().await;

        tracing::info!("Checking if provided term is accepted");

        if append_entries_input.term < raft_stable_data.current_term {
            tracing::warn!("Term provided has already been surpassed. Return failure and current term");

            return Ok(Response::new(AppendEntriesOutput {
                success: false,
                term: raft_stable_data.current_term
            }));
        };

        // As a base case, if the previous term is the first log entry is matches
        let prev_term_matches = append_entries_input.prev_log_index == 0 || raft_stable_data.log.get(append_entries_input.prev_log_index as usize)
            .map_or_else(
                || true,
                |val| append_entries_input.prev_log_term == val.term
            );

        if !prev_term_matches {
            tracing::info!("Term of previous log index does not match or does not exist. Return failure and prev log term");

            return Ok(Response::new(AppendEntriesOutput {
                success: false,
                term: raft_stable_data.log[raft_stable_data.log.len() - 1].term
            }))
        };

        let output = self.make_update_from_peer(
            raft_stable_data.deref_mut(),
            raft_volatile_data.deref_mut(),
            append_entries_input
        ).await.map_err(|_|
            Status::internal("Failed to update the state machine")
        )?;

        // TODO TODO TODO: Also start writing unit tests. Will make coming back to this way easier. Honestly probably possible to have unit tests
        //  spin up a few local servers and then just call each other to test the logic.

        return Ok(Response::new(output));
    }

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
}

// Helper functions
async fn parse_log_entries(
    raft_stable_data: &RaftStableData,
    input: ProposeValueInput,
) -> Result<Vec<LogEntry>, StateMachineError> {

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