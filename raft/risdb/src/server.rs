use std::{env, net::SocketAddr};

use raft_grpc::server::{RisDb, RisDbSetup};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    setup_tracing()?;

    // I don't think this will be forever but for now it lets multiple servers run
    // on local-host
    let args: Vec<_> = env::args().collect();

    let my_addr_unparsed = args.get(1).unwrap();
    let peer_addrs: Vec<String> = args[2..].into();


    let addr: SocketAddr = my_addr_unparsed.parse()?;
    let ris_db = RisDb { };

    let _ = ris_db.startup(addr, peer_addrs).await;

    Ok(())
}

fn setup_tracing() -> Result<(), Box<dyn std::error::Error>> {
    // construct a subscriber that prints formatted traces to stdout
    let subscriber = tracing_subscriber::fmt()
        // Use a more compact, abbreviated log format
        .compact()
        // Display source code file paths
        .with_file(true)
        // Display source code line numbers
        .with_line_number(true)
        // Display the thread ID an event was recorded on
        .with_thread_ids(false)
        // Don't display the event's target (module path)
        .with_target(false)
        // Build the subscriber
        .finish();
    // use that subscriber to process traces emitted after this point
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
