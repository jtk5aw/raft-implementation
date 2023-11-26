mod risdb_impl;
pub mod server;
mod raft;

pub mod raft_grpc {
    tonic::include_proto!("raftgrpc");
}

pub mod risdb {
    tonic::include_proto!("risdb");
}

pub mod shared {
    tonic::include_proto!("shared");
}
