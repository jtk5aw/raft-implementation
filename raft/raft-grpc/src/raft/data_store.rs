use crate::raft::peer::StateMachineError;
use crate::raft::state::{RaftStableData, RaftVolatileData};
use crate::raft_grpc::log_entry::LogAction;
use crate::raft_grpc::LogEntry;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default, Debug)]
pub struct DataStore {
    pub data: Arc<Mutex<HashMap<String, String>>>,
}

#[tonic::async_trait]
pub(crate) trait StateMachine {
    ///
    /// Check if last_applied < commit_index. As long as that statement is true, the log
    /// at last_applied + 1 will be applied to the state machine.
    ///
    /// Returns:
    /// Ok(()): All logs applied successfully and last_applied = commit_index now.
    /// Err(_): Some issue while attempting to apply logs to the state machine. last_applied is one less than
    ///         the index that causes the failure to occur. I.E if index 2 does not contain a value and can not be written
    ///         to the state machine, last_applied will be 1 when the error is returned.
    ///
    async fn update_state_machine(
        &self,
        stable_data: &RaftStableData,
        volatile_data: &mut RaftVolatileData,
    ) -> Result<i64, StateMachineError>;

    ///
    /// Takes a single log entry and applies it to the state machine.
    async fn apply_to_state_machine(
        &self,
        log_entry_to_apply: &LogEntry,
    ) -> Result<(), StateMachineError>;
}

#[tonic::async_trait]
impl StateMachine for DataStore {
    #[tracing::instrument(skip_all, ret, err(Debug))]
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

    #[tracing::instrument(skip_all, ret, err(Debug))]
    async fn apply_to_state_machine(
        &self,
        log_entry_to_apply: &LogEntry,
    ) -> Result<(), StateMachineError> {
        let mut data = self.data.lock().await;

        match LogAction::from_i32(log_entry_to_apply.log_action) {
            Some(LogAction::Put) => {
                let value = log_entry_to_apply.value.as_ref().ok_or_else(|| {
                    StateMachineError::FailedToApplyLogs(
                        "Failed to get log entries value".to_owned(),
                    )
                })?;

                data.insert(value.key.to_owned(), value.value.to_owned());

                Ok(())
            }
            Some(LogAction::Delete) => {
                todo!("Delete functionality is not yet implemented")
            }
            Some(LogAction::Noop) => {
                // do nothing
                Ok(())
            }
            None => Err(StateMachineError::FailedToApplyLogs(
                "No log action was stored".to_owned(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::raft::node::{Database, RaftImpl};
    use crate::shared::Value;

    use super::*;

    #[tokio::test]
    async fn noop_apply_to_state_machine() {
        let raft_impl = RaftImpl::new(Arc::new(Database::new("[::1]:5000".parse().unwrap())));
        let raft = raft_impl.inner;

        let expected_data = { raft.data_store.data.lock().await.clone() };

        let result = raft
            .data_store
            .apply_to_state_machine(&LogEntry {
                log_action: LogAction::Noop as i32,
                value: None,
                term: 0,
            })
            .await;

        let after_data = { raft.data_store.data.lock().await.clone() };

        assert!(
            result.is_ok(),
            "Noop log entry should not cause an error, result was `{:?}`",
            result
        );
        assert!(
            expected_data == after_data,
            "The two values were not equal `{:?}` after_data: `{:?}`",
            expected_data,
            after_data
        );
    }

    #[tokio::test]
    async fn put_apply_to_state_machine() {
        let raft_impl = RaftImpl::new(Arc::new(Database::new("[::1]:5000".parse().unwrap())));
        let raft = raft_impl.inner;

        let expected_data = {
            let mut original = raft.data_store.data.lock().await.clone();
            original.insert("key".to_owned(), "value".to_owned());
            original
        };

        let result = raft
            .data_store
            .apply_to_state_machine(&LogEntry {
                log_action: LogAction::Put as i32,
                value: Some(Value {
                    key: "key".to_owned(),
                    value: "value".to_owned(),
                }),
                term: 0,
            })
            .await;

        let after_data = { raft.data_store.data.lock().await.clone() };

        assert!(
            result.is_ok(),
            "Noop log entry should not cause an error, result was `{:?}`",
            result
        );
        assert!(
            expected_data == after_data,
            "The two values were not equal `{:?}` after_data: `{:?}`",
            expected_data,
            after_data
        );
    }

    #[tokio::test]
    async fn multiple_put_apply_to_state_machine() {
        let raft_impl = RaftImpl::new(Arc::new(Database::new("[::1]:5000".parse().unwrap())));
        let raft = raft_impl.inner;

        let expected_data = {
            let mut original = raft.data_store.data.lock().await.clone();
            original.insert("key".to_owned(), "value_2".to_owned());
            original
        };

        let _ = raft
            .data_store
            .apply_to_state_machine(&LogEntry {
                log_action: LogAction::Put as i32,
                value: Some(Value {
                    key: "key".to_owned(),
                    value: "value".to_owned(),
                }),
                term: 0,
            })
            .await;
        let result = raft
            .data_store
            .apply_to_state_machine(&LogEntry {
                log_action: LogAction::Put as i32,
                value: Some(Value {
                    key: "key".to_owned(),
                    value: "value_2".to_owned(),
                }),
                term: 0,
            })
            .await;

        let after_data = { raft.data_store.data.lock().await.clone() };

        assert!(
            result.is_ok(),
            "Noop log entry should not cause an error, result was `{:?}`",
            result
        );
        assert!(
            expected_data == after_data,
            "The two values were not equal `{:?}` after_data: `{:?}`",
            expected_data,
            after_data
        );
    }

    #[tokio::test]
    async fn none_apply_to_state_machine() {
        let raft_impl = RaftImpl::new(Arc::new(Database::new("[::1]:5000".parse().unwrap())));
        let raft = raft_impl.inner;

        let result = raft
            .data_store
            .apply_to_state_machine(&LogEntry {
                log_action: -1,
                value: None,
                term: 0,
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn two_noops_update_state_machine() {
        let raft_impl = RaftImpl::new(Arc::new(Database::new("[::1]:5000".parse().unwrap())));
        let raft = raft_impl.inner;

        {
            // setup stable data
            let mut raft_stable_data = raft.state.raft_data.lock().await;
            raft_stable_data.log.append(&mut vec![
                LogEntry {
                    log_action: LogAction::Noop as i32,
                    value: None,
                    term: 1,
                },
                LogEntry {
                    log_action: LogAction::Noop as i32,
                    value: None,
                    term: 1,
                },
            ]);

            // setup volatile data
            let mut raft_volatile_data = raft.volatile_state.raft_data.lock().await;
            raft_volatile_data.commit_index = 2;
        }

        let raft_stable_data = raft.state.raft_data.lock().await;
        let mut raft_volatile_data = raft.volatile_state.raft_data.lock().await;

        let result = raft
            .data_store
            .update_state_machine(&raft_stable_data, &mut raft_volatile_data)
            .await;

        assert!(
            result.is_ok(),
            "Noop log entries should not cause an error, result was `{:?}`",
            result
        );
        assert!(
            result.as_ref().unwrap() == &2,
            "Two noop log entries should be applied, result was `{:?}`",
            result
        );
        assert!(
            raft_volatile_data.last_applied == 2,
            "Raft volatile data was not updated as expected: raft_volatile_data `{:?}`",
            raft_volatile_data
        );
        let data_store = raft.data_store.data.lock().await;
        assert!(
            *data_store == HashMap::new(),
            "Raft data store should not have experienced changes from two Noops: data_store: {:?}",
            data_store
        )
    }

    #[tokio::test]
    async fn two_puts_update_state_machine() {
        let raft_impl = RaftImpl::new(Arc::new(Database::new("[::1]:5000".parse().unwrap())));
        let raft = raft_impl.inner;

        {
            // setup stable data
            let mut raft_stable_data = raft.state.raft_data.lock().await;
            raft_stable_data.log.append(&mut vec![
                LogEntry {
                    log_action: LogAction::Put as i32,
                    value: Some(Value {
                        key: "world".to_string(),
                        value: "hello".to_string(),
                    }),
                    term: 1,
                },
                LogEntry {
                    log_action: LogAction::Put as i32,
                    value: Some(Value {
                        key: "hello".to_string(),
                        value: "world".to_string(),
                    }),
                    term: 1,
                },
            ]);

            // setup volatile data
            let mut raft_volatile_data = raft.volatile_state.raft_data.lock().await;
            raft_volatile_data.commit_index = 2;
        }

        let raft_stable_data = raft.state.raft_data.lock().await;
        let mut raft_volatile_data = raft.volatile_state.raft_data.lock().await;

        let result = raft
            .data_store
            .update_state_machine(&raft_stable_data, &mut raft_volatile_data)
            .await;

        assert!(
            result.is_ok(),
            "Put log entries should not cause an error, result was `{:?}`",
            result
        );
        assert!(
            result.as_ref().unwrap() == &2,
            "Two put log entries should be applied, result was `{:?}`",
            result
        );
        assert!(
            raft_volatile_data.last_applied == 2,
            "Raft volatile data was not updated as expected: raft_volatile_data `{:?}`",
            raft_volatile_data
        );
        let data_store = raft.data_store.data.lock().await;
        let expected_data = HashMap::from([
            ("hello".to_string(), "world".to_string()),
            ("world".to_string(), "hello".to_string()),
        ]);
        assert!(
            *data_store == expected_data,
            "Raft data store should have experienced changes from two puts: data_store: {:?}",
            data_store
        )
    }

    #[tokio::test]
    async fn last_applied_equals_commit_update_state_machine() {
        let raft_impl = RaftImpl::new(Arc::new(Database::new("[::1]:5000".parse().unwrap())));
        let raft = raft_impl.inner;

        {
            // setup stable data
            let mut raft_stable_data = raft.state.raft_data.lock().await;
            raft_stable_data.log.append(&mut vec![
                LogEntry {
                    log_action: LogAction::Noop as i32,
                    value: None,
                    term: 1,
                },
                LogEntry {
                    log_action: LogAction::Noop as i32,
                    value: None,
                    term: 1,
                },
            ]);

            // setup volatile data
            let mut raft_volatile_data = raft.volatile_state.raft_data.lock().await;
            raft_volatile_data.commit_index = 0;
        }

        let raft_stable_data = raft.state.raft_data.lock().await;
        let mut raft_volatile_data = raft.volatile_state.raft_data.lock().await;

        let result = raft
            .data_store
            .update_state_machine(&raft_stable_data, &mut raft_volatile_data)
            .await;

        assert!(
            result.is_ok(),
            "No applied log entries so there shouldn't be an error, result was `{:?}`",
            result
        );
        assert!(
            result.as_ref().unwrap() == &0,
            "No put log entries should be applied, result was `{:?}`",
            result
        );
        assert!(
            raft_volatile_data.last_applied == 0,
            "Raft volatile data was not updated as expected: raft_volatile_data `{:?}`",
            raft_volatile_data
        );
        let data_store = raft.data_store.data.lock().await;
        assert!(
            *data_store == HashMap::new(),
            "Raft data store should not have experienced changes from two Noops: data_store: {:?}",
            data_store
        )
    }
}
