use std::{env, net::SocketAddr};

use raft_grpc::server::{PeerArgs, RisDb, RisDbSetup, ServerArgs};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    setup_tracing()?;

    let args: Vec<_> = env::args().collect();
    let my_args = args.get(1).unwrap();
    let server_args = parse_server_args(my_args.to_string())?;

    let ris_db = RisDb { };

    let _ = ris_db.startup(server_args).await;

    Ok(())
}

fn parse_server_args(arg: String) -> Result<ServerArgs, Box<dyn std::error::Error>> {
    let mut pairs = arg.split(";");

    let this_server_arg = pairs.next()
        .ok_or_else(|| "Failed to get server arg".to_string())?
        .split_once(',')
        .ok_or_else(|| "Failed to split server arg".to_string())?;
    let addr = this_server_arg.0.parse()?;
    let key_dir = format!(
        "{}/certs/keys_{}/",
        env!("CARGO_MANIFEST_DIR"),
        this_server_arg.1
    );

    let peer_args = pairs
        .map(|peer_arg_str| {
            let split_peer_arg = peer_arg_str.split_once(',').unwrap();
            PeerArgs {
                addr: split_peer_arg.0.to_string(),
                key_dir: format!(
                    "{}/certs/keys_{}/",
                    env!("CARGO_MANIFEST_DIR"), // TODO TODO TODO: The problem right now is this is package specfic. Need a good way of sharing across packages. 
                    split_peer_arg.1
                )
            }
        })
        .collect();

    Ok(ServerArgs {
        addr,
        key_dir,
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
