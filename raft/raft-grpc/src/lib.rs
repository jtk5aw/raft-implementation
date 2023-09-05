mod raft_impl;
pub mod server;

pub mod raft_grpc {
    tonic::include_proto!("raftgrpc");
}
