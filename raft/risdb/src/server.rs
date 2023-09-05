use std::env;

use raft_grpc::server::{RisDB, RisDBSetup};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    // TODO: create a second RPC service for outside clients to request in with that will request in

    // I don't think this will be forever but for now it lets multiple servers run 
    // on local-host
    let args: Vec<_> = env::args().collect();

    let my_port = args.get(1).unwrap();
    let client_port = args.get(2).unwrap();
    
    let addr = format!("[::1]:{}", my_port).parse()?;


    let _ = RisDB::startup(addr, vec![client_port.to_owned()]).await;
    
    Ok(())
}
