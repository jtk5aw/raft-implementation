use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::sleep;

use raft_grpc::database::{PeerArgs, RisDb, RisDbImpl, ServerArgs};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    setup_tracing()?;

    let args: Vec<_> = env::args().collect();
    let my_args = args.get(1).unwrap();
    let server_args = parse_server_args(my_args.to_string())?;

    let ris_db = RisDb::new(server_args.raft_addr);
    let _database = ris_db.database.clone();

    let _handle = ris_db.startup(server_args);

    // TODO: In case I get sidetracked by other stuff, I want that thin layer in front to be
    //  the stuff in risdb-hyper for now. Also ideally the raft stuff would communicate with each other 
    //  via TLS but I was at my limit trying to make that work with tonic   
    loop {
        // Imagine this is the hyper server
        sleep(Duration::from_secs(5));
    }
}

fn parse_server_args(arg: String) -> Result<ServerArgs, Box<dyn std::error::Error>> {
    let mut pairs = arg.split(";");

    let mut this_server_arg = pairs.next()
        .ok_or_else(|| "Failed to get server arg".to_string())?
        .split(',');
    let risdb_addr = this_server_arg.next()
        .ok_or("Did not have risdb addr")?
        .parse()?;
    let raft_addr = this_server_arg.next()
        .ok_or("Did not have raft addr")?
        .parse()?;

    let peer_args = pairs
        .map(|peer_arg_str| {
            let split_peer_arg = peer_arg_str.split_once(',').unwrap();
            PeerArgs {
                addr: split_peer_arg.0.to_string(),
            }
        })
        .collect();

    Ok(ServerArgs {
        risdb_addr,
        raft_addr,
        peer_args
    })
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
