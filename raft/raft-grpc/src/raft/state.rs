use std::sync::Arc;
use tokio::sync::Mutex;
use crate::raft_grpc::LogEntry;

#[derive(Debug, Clone)]
pub struct RaftStableState {
    pub raft_data: Arc<Mutex<RaftStableData>>,
}

#[derive(Debug, Clone)]
pub enum RaftNodeType {
    Follower(bool),
    Candidate,
    Leader,
    StartingUp,
}

#[derive(Debug)]
pub struct RaftStableData {
    pub node_type: RaftNodeType,
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