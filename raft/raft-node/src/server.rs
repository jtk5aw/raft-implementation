use std::env;
use std::time::Duration;

use tokio::task;
use tokio::time::sleep;
use tonic::transport::Server;

use raft_grpc::server::RaftImpl;
use raft_grpc::raft_grpc::raft_internal_server::RaftInternalServer;
use raft_grpc::raft_grpc::raft_internal_client::RaftInternalClient;
use raft_grpc::raft_grpc::AppendEntriesInput;


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    // TODO: create a second RPC service for outside clients to request in with that will request in

    // I don't think this will be forever but for now it lets multiple servers run 
    // on local-host
    let args: Vec<_> = env::args().collect();

    let my_port = args.get(1).unwrap();
    let client_port = args.get(2).unwrap();
    
    let addr = format!("[::1]:{}", my_port).parse()?;
    let raft = RaftImpl::default();

    task::spawn(async move {
        Server::builder()
            .add_service(RaftInternalServer::new(raft))
            .serve(addr)
            .await
    });

    println!("Printing before sleep");

    sleep(Duration::from_secs(5)).await;

    println!("Printing after sleep");

    let mut client = RaftInternalClient::connect(
        format!("https://[::1]:{}", client_port)
    ).await?;

    let request = tonic::Request::new(AppendEntriesInput {
        leader_id: my_port.parse().unwrap(), 
        term: 1,
        entry: vec![client_port.parse().unwrap()]
    });

    let response = client.append_entries(request).await;

    println!("RESPONSE={:?}", response.unwrap().into_inner().success);

    sleep(Duration::from_secs(5)).await;

    Ok(())
}
