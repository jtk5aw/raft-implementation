use std::{env, net::SocketAddr};

use opentelemetry::global;
use raft_grpc::server::{RisDb, RisDbSetup};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    // I don't think this will be forever but for now it lets multiple servers run 
    // on local-host
    let args: Vec<_> = env::args().collect();

    let my_port = args.get(1).unwrap();
    let client_port = args.get(2).unwrap();
    let client_port_2 = args.get(3).unwrap(); // quick spot check shows this can work. Left out for the time being
    
    let addr = format!("[::1]:{}", my_port).parse()?;
    let ris_db = RisDb { };

    setup_tracing(addr)?;

    let _ = ris_db.startup(addr, vec![client_port.to_owned(), client_port_2.to_owned()]).await;
    
    Ok(())
}

fn setup_tracing(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    // // construct a subscriber that prints formatted traces to stdout
    // let subscriber = tracing_subscriber::fmt()
    //     // Use a more compact, abbreviated log format
    //     .compact()
    //     // Display source code file paths
    //     .with_file(true)
    //     // Display source code line numbers
    //     .with_line_number(true)
    //     // Display the thread ID an event was recorded on
    //     .with_thread_ids(false)
    //     // Don't display the event's target (module path)
    //     .with_target(false)
    //     // Build the subscriber
    //     .finish();
    // // use that subscriber to process traces emitted after this point
    // tracing::subscriber::set_global_default(subscriber)?;

    // Allows you to pass along context (i.e., trace IDs) across services
    global::set_text_map_propagator(opentelemetry_jaeger::Propagator::new());
    // Sets up the machinery needed to export data to Jaeger
    // There are other OTel crates that provide pipelines for the vendors
    // mentioned earlier.
    let tracer = opentelemetry_jaeger::new_pipeline()
        .with_service_name(format!("risdb-{:?}", addr))
        .install_simple()?;

    // Create a tracing layer with the configured tracer
    let opentelemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    // let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
    //     .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // The SubscriberExt and SubscriberInitExt traits are needed to extend the
    // Registry to accept `opentelemetry (the OpenTelemetryLayer type).
    tracing_subscriber::registry()
        // .with(env_filter)
        .with(opentelemetry)
        // Continue logging to stdout
        .try_init()?;

    Ok(())
}
