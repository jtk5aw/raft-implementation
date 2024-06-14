use raft_grpc::database::{RisDb, RisDbImpl, ServerArgs};
use risdb_hyper::{get_workspace_base_dir, run};
use std::net::SocketAddr;
use std::str::FromStr;
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

    let addr_1 = SocketAddr::from_str("[::1]:3000").expect("Provided addr should be parseable");
    let addr_2 = SocketAddr::from_str("[::1]:3001").expect("Provided addr should be parseable");

    let base_dir = get_workspace_base_dir();
    let cert_path = &base_dir.join("certs").join("risdb.pem");
    let key_path = &base_dir.join("certs").join("risdb.ec");

    // Create dummy database that doesn't actually do anything without calling startup
    let risdb = RisDb::new(addr_2);

    run(addr_1, cert_path, key_path, risdb).await?;

    Ok(())
}
