// Having #[tracing] in #[tonic::async_trait] just marks the whole function as this
// making it useless
#![allow(clippy::blocks_in_conditions)]

pub mod database;
mod raft;

pub mod raft_grpc {
    tonic::include_proto!("raftgrpc");
}

pub mod shared {
    tonic::include_proto!("shared");
}
