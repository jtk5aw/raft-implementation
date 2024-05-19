use std::env;
use std::path::{Path, PathBuf};

use raft_grpc::server::{PeerArgs, RisDb, RisDbSetup, ServerArgs};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    setup_tracing()?;

    // Get workspace base
    let output = std::process::Command::new(env!("CARGO"))
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format=plain")
        .output()
        .unwrap()
        .stdout;
    let cargo_path = Path::new(std::str::from_utf8(&output).unwrap().trim());
    let workspace_base = cargo_path.parent().unwrap().to_path_buf();

    let args: Vec<_> = env::args().collect();
    let my_args = args.get(1).unwrap();
    let server_args = parse_server_args(workspace_base, my_args.to_string())?;

    let ris_db = RisDb { };

    let _ = ris_db.startup(server_args).await;

    Ok(())
}

fn parse_server_args(workspace_base: PathBuf, arg: String) -> Result<ServerArgs, Box<dyn std::error::Error>> {
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
