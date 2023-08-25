use tonic::{Request, Response, Status};

use raft_grpc::greeter_server::Greeter;
use raft_grpc::{HelloReply, HelloRequest};

pub mod raft_grpc {
    tonic::include_proto!("raftgrpc");
}

#[derive(Debug, Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self, 
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        println!("Got a request: {:?}", request);

        let reply = raft_grpc::HelloReply {
            message: format!("Hello {}!", request.into_inner().name).into(),
        };

        Ok(Response::new(reply))
    }
}


