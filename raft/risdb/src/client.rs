use raft_grpc::risdb::{PutRequest, GetRequest};
use raft_grpc::risdb::ris_db_client::RisDbClient;
use raft_grpc::shared::Value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = RisDbClient::connect("http://[::1]:50051").await?;
    let mut client_2 = RisDbClient::connect("http://[::1]:50052").await?;
    
    // Put request to first client
    let put_request = tonic::Request::new(PutRequest {
        request_id: "1".to_owned(),
        values: vec![ 
            Value {
                key: "key".to_owned(),
                value: "value".to_owned(),
            }
        ],
    });

    let put_response = client.put(put_request).await?;

    println!("RESPONSE={:?}", put_response);

    // Get Request to first client
    let get_request = tonic::Request::new(GetRequest {
        request_id: "2".to_owned(),
        keys: vec![
            "key".to_owned()
        ],
    });

    let get_response = client.get(get_request).await?;

    println!("RESPONSE={:?}", get_response);

    // Get reqeust to second client
    let get_request = tonic::Request::new(GetRequest {
        request_id: "3".to_owned(),
        keys: vec![
            "key".to_owned()
        ],
    });

    let get_response = client_2.get(get_request).await?;

    println!("RESPONSE_2={:?}", get_response);

    // Get reqeust to second client key does not exist
    let get_request = tonic::Request::new(GetRequest {
        request_id: "4".to_owned(),
        keys: vec![
            "fake".to_owned()
        ],
    });

    let get_response = client_2.get(get_request).await?;

    println!("RESPONSE_3={:?}", get_response);
    
    Ok(())
}
