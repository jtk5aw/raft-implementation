use raft_grpc::raft_grpc::raft_internal_client::RaftInternalClient;
use raft_grpc::raft_grpc::AppendEntriesInput;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = RaftInternalClient::connect("http://[::1]:50051").await?;
    
    let request = tonic::Request::new(AppendEntriesInput {
        leader_id: 1,
        term: 1, 
        entry: vec!["hello".to_owned()],
    });

    let response = client.append_entries(request).await?;

    println!("RESPONSE={:?}", response);
    
    Ok(())
}
