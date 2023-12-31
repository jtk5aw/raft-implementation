use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::raft::node::RaftImpl;
use crate::raft::state::{RaftStableData, RaftVolatileData};
use crate::raft_grpc::log_entry::LogAction;
use crate::raft_grpc::LogEntry;
use crate::raft::peer::StateMachineError;

// TODO: Long term consider decoupling the Raft Node and the data store even more.
//  this isn't really any form of abstraction right now (and that makes things easier)
//  but any sort of complicated data store would serve well from abstracting this better

#[derive(Debug, Clone)]
pub struct DataStore {
    pub data: Arc<Mutex<HashMap<String, String>>>,
}

#[tonic::async_trait]
pub trait StateMachine {

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
    async fn update_state_machine(
        &self,
        stable_data: &RaftStableData,
        volatile_data: &mut RaftVolatileData,
    ) -> Result<i64, StateMachineError>;

    /**
     * Takes a single log entry and applies it to the state machine.
     */
    async fn apply_to_state_machine(
        &self,
        log_entry_to_apply: &LogEntry,
    ) -> Result<(), StateMachineError>;
}

#[tonic::async_trait]
impl StateMachine for RaftImpl {

    async fn update_state_machine(
        &self,
        stable_data: &RaftStableData,
        volatile_data: &mut RaftVolatileData,
    ) -> Result<i64, StateMachineError> {

        let mut num_updates = 0;

        while volatile_data.last_applied < volatile_data.commit_index {

            let index_to_apply = volatile_data.last_applied + 1;

            let log_entry_to_apply = &stable_data.log[index_to_apply as usize];

            self.apply_to_state_machine(log_entry_to_apply).await?;

            volatile_data.last_applied += 1;
            num_updates += 1;
        }

        Ok(num_updates)
    }

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
            Some(LogAction::Noop) => {
                // do nothing
                Ok(())
            },
            None => {
                Err(StateMachineError::FailedToApplyLogs("No log action was stored".to_owned()))
            }
        }
    }
}