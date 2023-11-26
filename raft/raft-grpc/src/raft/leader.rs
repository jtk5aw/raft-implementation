use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::timeout;
use crate::raft::node::{RaftImpl, StateMachineError};
use crate::raft::node::StateMachineError::FailedToWriteToLogs;
use crate::raft::peer::PeerData;
use crate::raft::state::{RaftStableData, RaftVolatileData};
use crate::raft_grpc::{AppendEntriesInput, LogEntry};

// Errors

#[derive(Debug)]
pub enum LeaderError {
    NoLeaderStateFound(String),
    NoMinimumPeerCommitIndexFound(String),
    NotEnoughPeersUpdated(String),
    FailedToAppendEntries(String),
}

// Method Return Structs
struct ProposeToPeersResult {
    count_communicated: i64,
    request_append_entries_results: Vec<Result<i64, LeaderError>>
}

#[tonic::async_trait]
pub trait LeaderActions {

    /**
     * Makes requests to all peers and track how many succeed.
     *
     * ProposeToPeersResult => Object containing both the number of peers that were successfully communicated to
     * and the results of communicating to every peer. Both in successful and unsuccessful cases.
     */
    async fn share_to_peers(
        &self,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>
    ) -> ProposeToPeersResult;

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
    ) -> Result<i64, LeaderError>;

    /**
     * Adds the provided LogEntry values to the logs of self and peers. Also checks if
     * data was shared to enough peers to be considered committed.
     *
     * Returns:
     * OK(()): Successfully updated logs for self and a majority of servers overall.
     * Err(String): Failed to update logs of a majority of servers overall.
     */
    async fn write_and_share_logs(
        &self,
        stable_data: &mut RaftStableData,
        volatile_data: &mut RaftVolatileData,
        log_entries: Vec<LogEntry>
    ) -> Result<(), StateMachineError>;

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
        peer_data: &mut PeerData,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>,
    ) -> Result<i64, LeaderError>;
}

#[tonic::async_trait]
impl LeaderActions for RaftImpl {
    async fn share_to_peers(
        &self,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>
    ) -> ProposeToPeersResult {

        let count_communicated: Arc<Mutex<i64>> = Arc::new(Mutex::new(1)); // Set to 1 to include self
        let request_append_entries_results: Vec<Result<i64, LeaderError>> = {
            futures::future::join_all(self.peer_connections.peers.lock().await
                .deref_mut()
                .iter_mut()
                .map(|(peer_addr, peer_data)| self.request_append_entries_peer(
                    count_communicated.clone(),
                    peer_addr.to_owned(),
                    peer_data,
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
                        "Failed to get minimum peer commit index after successful communication to the majority".to_owned()
                    ))?
            )
        } else {
            Err(LeaderError::NotEnoughPeersUpdated(
                "Not enough peers were updated to get a new commit index".to_owned()
            ))
        }
    }

    async fn write_and_share_logs(
        &self,
        stable_data: &mut RaftStableData,
        volatile_data: &mut RaftVolatileData,
        log_entries: Vec<LogEntry>
    ) -> Result<(), StateMachineError> {

        tracing::info!("Writing new value to own log");

        log_entries
            .iter()
            .for_each(|log_entry| {
                stable_data.log.push(log_entry.to_owned());
            });

        // TODO: Continue retrying to reach peers that failed and also handle decrementing log entry and retrying for log inconsistencies
        tracing::info!("Proposing new value to peers");

        let propose_to_peers_result = self.share_to_peers(
            volatile_data.commit_index,
            &log_entries
        ).await;

        tracing::info!("Calculate the new commit index");

        match self.calculate_new_commit_index(
            propose_to_peers_result.count_communicated,
            propose_to_peers_result.request_append_entries_results
        ).await {
            Ok(min_new_commit_index) => {
                volatile_data.commit_index = min_new_commit_index;
                Ok(())
            },
            Err(_) => {
                tracing::error!("Data not committed not enough peers received updates");
                Err(FailedToWriteToLogs("Data not committed, not enough peers received updates".to_owned()))
            }
        }?;

        Ok(())
    }

    async fn request_append_entries_peer(
        &self,
        count_communicated: Arc<Mutex<i64>>,
        peer_addr: String,
        peer_data: &mut PeerData,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>,
    ) -> Result<i64, LeaderError> {
        let peer_leader_state = peer_data.leader_state
            .as_mut()
            .ok_or_else(|| LeaderError::NoLeaderStateFound("No leader state found".to_owned()))?;

        let peer_leader_connection = peer_data.connection
            .as_mut()
            .ok_or_else(|| LeaderError::NoLeaderStateFound("No leader state found".to_owned()))?;

        // Make request to append entries TODO: Play with the timeout time here
        let request_result = match timeout(Duration::from_secs(5), peer_leader_connection.append_entries(AppendEntriesInput {
            leader_id: self.addr.to_string(),
            term: 0, // TODO: Actually handle different terms
            entries: log_entries.to_owned(),
            prev_log_index: (peer_leader_state.next_index - 1),
            prev_log_term: 0, // TODO: Actually handle different terms
            leader_commit: curr_commit_index,
        })).await {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(?e, ?peer_addr, "Call to peer timed out");
                Err(LeaderError::FailedToAppendEntries(peer_addr.to_owned()))?
            }
        };

        match request_result {
            Ok(res) => {
                let append_entries_output = res.into_inner();

                if append_entries_output.success {
                    tracing::info!(?peer_addr, "Successfully appended entries to peer");

                    peer_leader_state.next_index += log_entries.len() as i64;
                    peer_leader_state.match_index += log_entries.len() as i64;

                    if peer_leader_state.match_index >= curr_commit_index {
                        tracing::info!("Peer now has a match index where all new logs are applied. Updated count of peers communicated to");
                        *count_communicated.lock().await += 1;
                    }
                    Ok(peer_leader_state.match_index)
                } else {
                    todo!("Handle this case where the peer responded but with success set as false. means need to retry based on term provided back");
                }
            },
            Err(e) => {
                tracing::error!(?e, ?peer_addr, "Failed to append entries to peer");
                Err(LeaderError::FailedToAppendEntries(peer_addr.to_owned()))
            }
        }
    }
}
