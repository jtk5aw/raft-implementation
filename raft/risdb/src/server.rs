use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::join;

use raft_grpc::database::{PeerArgs, RisDb, RisDbImpl, ServerArgs};
use risdb_hyper::{get_workspace_base_dir, RisDbArgs, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing()?;

    let args: Vec<_> = env::args().collect();
    let my_args = args.get(1).unwrap();
    let server_args = generate_args(my_args.to_string())?;

    let ris_db = RisDb::new(server_args.raft_addr);

    let database_handle = ris_db.startup(server_args.clone().into());
    let frontend_handle = run(server_args.clone().into(), ris_db);

    join!(
        database_handle,
        frontend_handle
    );

    Ok(())
}

impl From<Args> for ServerArgs {
    fn from(value: Args) -> Self {
        return ServerArgs {
            raft_addr: value.raft_addr.to_owned(),
            peer_args: value.peer_args,
        }
    }
}

impl<'a> From<Args> for RisDbArgs {
    fn from(value: Args) -> Self {
        return RisDbArgs {
            addr: value.frontend_addr,
            cert_path: value.cert_path,
            key_path: value.key_path,
        }
    }
}

#[derive(Clone)]
struct Args {
    // Frontend args only
    frontend_addr: String,
    cert_path: PathBuf,
    key_path: PathBuf,
    // Raft args
    pub raft_addr: SocketAddr,
    pub peer_args: Vec<PeerArgs>,
}

fn generate_args<'a>(arg: String) -> Result<Args, Box<dyn std::error::Error>> {
    let mut pairs = arg.split(";");

    let mut this_server_arg = pairs
        .next()
        .ok_or_else(|| "Failed to get server arg".to_string())?
        .split(',');
    let frontend_addr = this_server_arg
        .next()
        .ok_or("Did not have risdb addr")?
        .parse()?;
    let raft_addr = this_server_arg
        .next()
        .ok_or("Did not have raft addr")?
        .parse()?;

    let base_dir = get_workspace_base_dir();
    let cert_path = base_dir.join("certs").join("risdb.pem");
    let key_path = base_dir.join("certs").join("risdb.ec");

    let peer_args = pairs
        .map(|peer_arg_str| {
            PeerArgs {
                addr: peer_arg_str.to_string(),
            }
        })
        .collect();

    Ok(Args {
        frontend_addr,
        cert_path,
        key_path,
        raft_addr,
        peer_args,
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
