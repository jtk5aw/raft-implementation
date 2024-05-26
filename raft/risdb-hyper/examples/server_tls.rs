use std::net::SocketAddr;
use std::str::FromStr;
use tokio::try_join;
use risdb_hyper::{get_workspace_base_dir, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr_1 = SocketAddr::from_str("[::1]:3000").expect("Provided addr should be parseable");
    let addr_2 = SocketAddr::from_str("[::1]:3001").expect("Provided addr should be parseable");

    let base_dir = get_workspace_base_dir();
    let cert_path = &base_dir
        .join("certs")
        .join("risdb.pem");
    let key_path = &base_dir
        .join("certs")
        .join("risdb.ec");

    let _ = try_join!(
        run(addr_1, cert_path, key_path),
        run(addr_2, cert_path, key_path)
    )?;

    Ok(())
}