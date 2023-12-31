use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tonic::Request;
use tonic::transport::Channel;
use crate::raft_grpc::{PingInput, LogEntry, AppendEntriesInput, RequestVoteInput, RequestVoteOutput};
use crate::raft_grpc::raft_internal_client::RaftInternalClient;
use crate::raft::state::{RaftStableData, RaftVolatileData};

// Errors
#[derive(Debug)]
pub enum LeaderError {
    NoLeaderStateFound(String),
    NoMinimumPeerCommitIndexFound(String),
    NotEnoughPeersUpdated(String),
    FailedToAppendEntries(String),
}

#[derive(Debug)]
pub enum StateMachineError {
    FailedToWriteToLogs(String),
    FailedToApplyLogs(String),
}

pub enum CandidateError {
    NoClientFound(String),
    RequestForVoteFailure(String),
    CustomError(String),
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

#[derive(Debug)]
pub struct ProposeToPeersResult {
    count_communicated: i64,
    request_append_entries_results: Vec<Result<i64, LeaderError>>
}

pub struct RequestVotesResult {
    pub won_election: bool
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
    ) -> ();
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

// TODO: Determine how to prevent "public" from using this
#[tonic::async_trait]
pub trait LeaderActions {
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
        addr: String,
        stable_data: &mut RaftStableData,
        volatile_data: &mut RaftVolatileData,
        log_entries: Vec<LogEntry>
    ) -> Result<(), StateMachineError>;

    /**
     * Makes requests to all peers and track how many succeed.
     *
     * ProposeToPeersResult => Object containing both the number of peers that were successfully communicated to
     * and the results of communicating to every peer. Both in successful and unsuccessful cases.
     */
    async fn share_to_peers(
        &self,
        addr: String,
        stable_data: &RaftStableData,
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
     * Make a request to a peer to append entries.
     *
     * Returns:
     * OK(()): Successfully appended entries
     * Err(String): Failed to append entries for the provided address (the address is the key of the peer)
     */
    async fn request_append_entries_peer(
        &self,
        addr: String,
        raft_stable_data: &RaftStableData,
        count_communicated: Arc<Mutex<i64>>,
        peer_addr: String,
        peer_data: &mut PeerData,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>,
    ) -> Result<i64, LeaderError>;
}

#[tonic::async_trait]
impl LeaderActions for PeerConnections {
    async fn write_and_share_logs(
        &self,
        addr: String,
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
            addr,
            stable_data,
            volatile_data.commit_index,
            &log_entries
        ).await;

        tracing::info!(?propose_to_peers_result, "Calculate the new commit index");

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
                Err(StateMachineError::FailedToWriteToLogs("Data not committed, not enough peers received updates".to_owned()))
            }
        }?;

        Ok(())
    }

    async fn share_to_peers(
        &self,
        addr: String,
        raft_stable_data: &RaftStableData,
        curr_commit_index: i64,
        log_entries: &Vec<LogEntry>
    ) -> ProposeToPeersResult {

        let count_communicated: Arc<Mutex<i64>> = Arc::new(Mutex::new(1)); // Set to 1 to include self
        let request_append_entries_results: Vec<Result<i64, LeaderError>> = {
            futures::future::join_all(self.peers.lock().await
                .deref_mut()
                .iter_mut()
                .map(|(peer_addr, peer_data)| self.request_append_entries_peer(
                    addr.to_string(),
                    raft_stable_data,
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
        let peers = self.peers.lock().await;
        if count_communicated > (peers.len() / 2) as i64 {
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

    async fn request_append_entries_peer(
        &self,
        addr: String,
        raft_stable_data: &RaftStableData,
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
            leader_id: addr.to_string(),
            term: raft_stable_data.current_term, 
            entries: log_entries.to_owned(),
            prev_log_index: (raft_stable_data.log.len() - 1) as i64,
            prev_log_term: raft_stable_data.log.last().expect("Log should never be an empty list").term,
            leader_commit: curr_commit_index,
        })).await {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(?e, ?peer_addr, "Call to append entires to peer timed out");
                Err(LeaderError::FailedToAppendEntries(peer_addr.to_owned()))?
            }
        };

        match request_result {
            Ok(res) => {
                let append_entries_output = res.into_inner();

                if append_entries_output.success {
                    tracing::info!(?peer_addr, "Successfully appended entries to peer");

                    peer_leader_state.next_index = raft_stable_data.log.len()  as i64;
                    peer_leader_state.match_index = (raft_stable_data.log.len() - 1) as i64;

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

#[tonic::async_trait]
pub trait CandidateActions {

    /**
     * Requests votes from peers. Asynchronous calls to all peers are made. Tallies up results and determines
     * if the election was won or lost
     * 
     * Returns: 
     * RequestVotesResult: Result of the election
     */
    async fn request_votes(
        &self,
        addr: String,
        raft_stable_data: &RaftStableData
    ) -> RequestVotesResult;

    /**
     * Makes a request for vote in election to make this node leader to one peer. 
     * 
     * Returns: 
     * Ok(bool): True if peer voted for this node. False if not
     * Err(CandidateError): Reason that request to this peer failed. 
     */
    async fn request_vote_from_peer(
        &self,
        addr: String,
        peer_addr: String,
        peer_data: &mut PeerData,
        raft_stable_data: &RaftStableData
    ) -> Result<bool, CandidateError>;

    /**
     * Determine if the current node received enough yes votes to become the new leader. 
     * 
     * Returns: 
     * bool: True if this node should convert to leader. False if not
     */
    async fn determine_election_result(
        &self,
        count_voted_yes: i64,
    ) -> bool;
}

#[tonic::async_trait]
impl CandidateActions for PeerConnections {
    async fn request_votes(
        &self,
        addr: String,
        raft_stable_data: &RaftStableData
    ) -> RequestVotesResult {
        let results = {
            futures::future::join_all(self.peers.lock().await
                .deref_mut()
                .iter_mut()
                .map(|(peer_addr, peer_data)| self.request_vote_from_peer(
                    addr.to_owned(), 
                    peer_addr.to_owned(), 
                    peer_data, 
                    raft_stable_data
                ))
            ).await
        };

        let count_peers_voted_yes = results
            .into_iter()
            .filter(|result| result
                .as_ref()
                .is_ok_and(|voted_for| voted_for.to_owned())
            )
            .count() as i64;
        let count_voted_yes = count_peers_voted_yes + 1; // Add 1 to include self

        let won_election = self.determine_election_result(count_voted_yes).await;

        tracing::info!(count_voted_yes, won_election, addr, "Election result values for debugging");

        RequestVotesResult {
            won_election
        }
    }

    async fn request_vote_from_peer(
        &self,
        addr: String,
        peer_addr: String,
        peer_data: &mut PeerData,
        raft_stable_data: &RaftStableData
    ) -> Result<bool, CandidateError> {
        let peer_connection = peer_data.connection
        .as_mut()
        .ok_or_else(|| CandidateError::NoClientFound(format!("Raft Client found for {:?}", peer_addr)))?;

        let last_log_index = (raft_stable_data.log.len() - 1) as i64;
        let last_log_term = raft_stable_data.log.last().expect("Log should always be at least length 1").term;

        let request_vote_result = match timeout(Duration::from_secs(5), peer_connection.request_vote(RequestVoteInput {
            term: raft_stable_data.current_term,
            candidate_id: addr.to_string(),
            last_log_index,
            last_log_term,
        })).await {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(?e, ?peer_addr, "Call to request votes from peer timed out");
                Err(CandidateError::RequestForVoteFailure(format!("Failed to request vote from peer {:?}", peer_addr)))?
            }
        };

        match request_vote_result {
            Ok(res) => {
                let request_vote_output = res.into_inner();
                Ok(request_vote_output.vote_granted)
            },
            Err(e) => {
                tracing::error!(?e, ?peer_addr, "Failed to request vote from peer");
                Err(CandidateError::RequestForVoteFailure(format!("Failed to request vote from peer {:?}", peer_addr)))
            }
        }
    }

    async fn determine_election_result(
        &self,
        count_voted_yes: i64,
    ) -> bool {
        if count_voted_yes > (self.peers.lock().await.len() / 2) as i64 {
            tracing::info!("Candidate now has enough votes to become a leader");
            true
        } else {
            tracing::info!("Candidate does not have enough votes to become a leader");
            false
        }
    }

}