use std::cmp::min;
use crate::raft::data_store::StateMachine;
use crate::raft::node::{RaftImpl, StateMachineError};
use crate::raft::state::{RaftStableData, RaftVolatileData};
use crate::raft_grpc::{AppendEntriesInput, AppendEntriesOutput, LogEntry};

#[tonic::async_trait]
pub trait FollowerActions {
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
    async fn write_to_logs(
        &self,
        stable_data: &mut RaftStableData,
        log_entries: Vec<LogEntry>
    );
}

#[tonic::async_trait]
impl FollowerActions for RaftImpl {
    async fn make_update_from_peer(
        &self,
        raft_stable_data: &mut RaftStableData,
        raft_volatile_data: &mut RaftVolatileData,
        append_entries_input: AppendEntriesInput,
    ) -> Result<AppendEntriesOutput, StateMachineError> {
        tracing::info!("Clear the log from the provided index on of any entries not matching the provided term");

        raft_stable_data.log.truncate((append_entries_input.prev_log_index + 1) as usize);

        tracing::info!("Appending new entries to the log");

        self.write_to_logs(
            raft_stable_data,
            append_entries_input.entries
        ).await;

        if raft_volatile_data.commit_index < append_entries_input.leader_commit {
            tracing::info!("Updating commit index");
            raft_volatile_data.commit_index = min(
                (raft_stable_data.log.len() - 1) as i64,
                append_entries_input.leader_commit
            );
        }

        tracing::info!("Attempting to update state machine");

        match self.update_state_machine(
            raft_stable_data,
            raft_volatile_data
        ).await {
            Ok(num_updates) => {
                tracing::info!(?num_updates, "Successful updated the state machine");
                Ok(AppendEntriesOutput {
                    success: true,
                    term: 0, // TODO: Make this handle term properly and confirm other aspects of this method handle term correctly
                })
            },
            Err(err) => {
                tracing::error!(err = ?err, "Failed to update the state machine");
                Err(err)
            }
        }
    }

    async fn write_to_logs(
        &self,
        stable_data: &mut RaftStableData,
        log_entries: Vec<LogEntry>
    ) {
        tracing::info!("Writing new value to own log");

        log_entries
            .iter()
            .for_each(|log_entry| {
                stable_data.log.push(log_entry.to_owned());
            });
    }
}