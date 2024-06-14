// Having #[tracing] in #[tonic::async_trait] just marks the whole function as this
// making it useless
#![allow(clippy::blocks_in_conditions)]

pub mod database;
mod raft;
pub use self::raft::node::Database;
pub use self::raft::node::ReadAndWrite;

pub mod structs {
    tonic::include_proto!("raftgrpc");
    pub use prost::Message as RaftMessage;
}

pub mod shared {
    tonic::include_proto!("shared");
}
