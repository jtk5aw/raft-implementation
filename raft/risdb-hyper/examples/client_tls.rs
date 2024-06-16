use risdb_hyper::structs::{GetRequest, PutRequest, Value};
use risdb_hyper::{get_workspace_base_dir, ClientBuilder};
use tracing::Level;

fn setup_tracing() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // construct a subscriber that prints formatted traces to stdout
    let subscriber = tracing_subscriber::fmt()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(false)
        .with_target(false)
        .with_max_level(Level::INFO)
        .finish();
    // use that subscriber to process traces emitted after this point
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    setup_tracing()?;

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

    let put_result = client
        .put(vec![Value {
            key: "key".to_string(),
            value: "value".to_string(),
        }])
        .await;

    println!("{:?}", put_result);

    let get_result = client.get(vec!["key1".to_string()]).await;

    println!("{:?}", get_result);

    Ok(())
}
