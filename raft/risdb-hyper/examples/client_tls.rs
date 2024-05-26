use risdb_hyper::items::GetRequest;
use risdb_hyper::{get_workspace_base_dir, ClientBuilder, Get};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // This is where we will setup our HTTP client requests.

    let base_dir = get_workspace_base_dir();
    // Load root cert
    let base_cert_path = base_dir.join("certs").join("ca.cert");

    // Parse our URL...
    let url = "https://localhost:3000".parse::<hyper::Uri>()?;

    let client = ClientBuilder::new()
        .with_base_uri(url)
        .with_root_cert_path(base_cert_path)
        .build()
        .await
        .unwrap();

    let result = client
        .get(GetRequest {
            request_id: "1-1-1-1".to_string(),
            keys: vec!["key1".to_string()],
        })
        .await;

    println!("{:?}", result);

    Ok(())
}
