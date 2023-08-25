use tonic::transport::Server;

use raft_grpc::server::MyGreeter;
use raft_grpc::raft_grpc::greeter_server::GreeterServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse()?;
    let greeter = MyGreeter::default();

    Server::builder()
        .add_service(GreeterServer::new(greeter))
        .serve(addr)
        .await?;

    Ok(())
}
