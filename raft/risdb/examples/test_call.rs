use risdb_hyper::structs::Value;
use risdb_hyper::{get_workspace_base_dir, ClientBuilder, RisDbClient};
use std::env;
use tracing::{info, Level};

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

    let args: Vec<_> = env::args().collect();

    let url = args.get(1).unwrap();
    let action = args.get(2).unwrap().as_ref();

    // This is where we will setup our HTTP client requests.
    let base_dir = get_workspace_base_dir();
    // Load root cert
    let base_cert_path = base_dir.join("certs").join("ca.cert");

    // Parse our URL...
    let url = url.parse::<hyper::Uri>()?;

    let client = ClientBuilder::new()
        .with_base_uri(url)
        .with_root_cert_path(base_cert_path)
        .build()
        .await
        .unwrap();

    let result = match action {
        "GET" => make_get_call(client, args.get(3).unwrap().as_ref()).await,
        "PUT" => make_put_call(client, args.get(3).unwrap().as_ref()).await,
        _ => panic!("either input GET or PUT"),
    };

    info!("The returned result is: {:?}", result);

    Ok(())
}

async fn make_get_call(client: RisDbClient, input: &str) -> String {
    let keys: Vec<String> = input.split(',').map(|str| str.to_owned()).collect();

    let get_result = client.get(keys).await;

    format!("Returned result is: {:?}", get_result)
}

async fn make_put_call(client: RisDbClient, input: &str) -> String {
    let values = input
        .split(',')
        .map(|str| str.split_once(';').unwrap())
        .map(|pair| Value {
            key: pair.0.to_string(),
            value: pair.1.to_string(),
        })
        .collect();

    let put_result = client.put(values).await;

    format!("Returned result is: {:?}", put_result)
}
