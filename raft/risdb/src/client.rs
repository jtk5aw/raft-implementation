use tonic::transport::{Certificate, Channel, ClientTlsConfig};
use raft_grpc::risdb::GetRequest;
use raft_grpc::risdb::ris_db_client::RisDbClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // let mut client = RisDbClient::connect("http://[::1]:50053").await?;
    // let mut client_2 = RisDbClient::connect("http://[::1]:6141").await?;

    let pem = std::fs::read_to_string("/Users/jacksonkennedy/Documents/raft-implementation/raft/risdb-proxy/certs/keys/key.pem")?;
    let ca = Certificate::from_pem(pem);

    let tls = ClientTlsConfig::new()
        .ca_certificate(ca)
        .domain_name("localhost");

    let channel = Channel::from_static("https://[::1]:50051")
        .tls_config(tls)?
        .connect()
        .await?;

    let mut client_3 = RisDbClient::new(channel);

    // // Put request to first client
    // let put_request = tonic::Request::new(PutRequest {
    //     request_id: "1".to_owned(),
    //     values: vec![
    //         Value {
    //             key: "key".to_owned(),
    //             value: "value".to_owned(),
    //         }
    //     ],
    // });

    // println!("Testing with the old client");

    // let put_response = client.put(put_request).await?;

    // println!("RESPONSE={:?}", put_response);

    // // Get Request to first client
    // let get_request = tonic::Request::new(GetRequest {
    //     request_id: "2".to_owned(),
    //     keys: vec![
    //         "key".to_owned()
    //     ],
    // });

    // let get_response = client.get(get_request).await?;

    // println!("RESPONSE={:?}", get_response);

    // // Get reqeust to second client
    // let get_request = tonic::Request::new(GetRequest {
    //     request_id: "3".to_owned(),
    //     keys: vec![
    //         "key".to_owned()
    //     ],
    // });

    // println!("Testing with the new tcp client");

    // let get_response = client_2.get(get_request).await?;

    // println!("RESPONSE_2={:?}", get_response);

    // // Get reqeust to second client key does not exist
    // let get_request = tonic::Request::new(GetRequest {
    //     request_id: "4".to_owned(),
    //     keys: vec![
    //         "fake".to_owned()
    //     ],
    // });

    // let get_response = client_2.get(get_request).await?;

    // println!("RESPONSE_3={:?}", get_response);

    println!("Testing with a new tls client");

    let get_request = tonic::Request::new(GetRequest {
        request_id: "3".to_owned(),
        keys: vec![
            "key".to_owned()
        ],
    });

    let get_response = client_3.get(get_request).await?;

    println!("RESPONSE_2={:?}", get_response);

    // Get reqeust to second client key does not exist
    let get_request = tonic::Request::new(GetRequest {
        request_id: "4".to_owned(),
        keys: vec![
            "fake".to_owned()
        ],
    });

    let get_response = client_3.get(get_request).await?;

    println!("RESPONSE_3={:?}", get_response);

    Ok(())
}
