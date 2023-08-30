use tonic::{Request, Response, Status};

use super::raft_grpc::raft_internal_server::RaftInternal;
use super::raft_grpc::{AppendEntriesInput, AppendEntriesOutput};

#[derive(Debug, Default)]
pub struct RaftImpl {}

#[tonic::async_trait]
impl RaftInternal for RaftImpl {
    async fn append_entries(
        &self, 
        request: Request<AppendEntriesInput>,
    ) -> Result<Response<AppendEntriesOutput>, Status> {
        println!("Got a request: {:?}", request);

        let reply = AppendEntriesOutput {
            success: true,
            term: request.into_inner().term,
        };

        Ok(Response::new(reply))
    }
}


