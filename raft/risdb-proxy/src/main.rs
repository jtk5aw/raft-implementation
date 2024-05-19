use async_trait::async_trait;
use log::info;
use pingora::{apps::ServerApp, connectors::TransportConnector, listeners::{Listeners, TlsSettings}, protocols::Stream, server::{configuration::Opt, Server, ShutdownWatch}, services::{listening::Service, Service as OtherService}, upstreams::peer::HttpPeer};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use structopt::StructOpt;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, select};
use std::{fs::{create_dir_all, File}, io::Write, sync::Arc};
use pingora_core::protocols::ALPN;
use pingora_core::upstreams::peer::Peer;

pub struct ProxyApp {
    client_connector: TransportConnector,
    proxy_to: HttpPeer,
}

enum DuplexEvent {
    DownstreamRead(usize),
    UpstreamRead(usize),
}

impl ProxyApp {
    pub fn new(proxy_to: HttpPeer) -> Self {
        ProxyApp {
            client_connector: TransportConnector::new(None),
            proxy_to,
        }
    }

    async fn duplex(&self, mut server_session: Stream, mut client_session: Stream) -> Option<Stream> {
        let mut upstream_buf = [0; 1024];
        let mut downstream_buf = [0; 1024];
        loop {
            let downstream_read = server_session.read(&mut upstream_buf);
            let upstream_read = client_session.read(&mut downstream_buf);
            let event: DuplexEvent;
            select! {
                n = downstream_read => {
                    if n.is_err() {
                        continue;
                    }
                    event = DuplexEvent::DownstreamRead(n.unwrap())
                },
                n = upstream_read => {
                    event = DuplexEvent::UpstreamRead(n.unwrap())
                },
            }
            match event {
                DuplexEvent::DownstreamRead(0) => {
                    info!("downstream session closing");
                    return None;
                }
                DuplexEvent::UpstreamRead(0) => {
                    info!("upstream session closing");
                    return None;
                }
                DuplexEvent::DownstreamRead(n) => {
                    client_session.write_all(&upstream_buf[0..n]).await.unwrap();
                    client_session.flush().await.unwrap();
                }
                DuplexEvent::UpstreamRead(n) => {
                    server_session
                        .write_all(&downstream_buf[0..n])
                        .await
                        .unwrap();
                    server_session.flush().await.unwrap();
                }
            }
        }
    }
}


#[async_trait]
impl ServerApp for ProxyApp {
    async fn process_new(
        self: &Arc<Self>,
        io: Stream,
        _shutdown: &ShutdownWatch,
    ) -> Option<Stream> {
        let reused_session = self.client_connector
            .reused_stream(&self.proxy_to)
            .await;

        match reused_session {
            Some(client_session) => {
                info!("Reusing a connection");
                return self.duplex(io, client_session).await;
            }
            None => {
                info!("No session to re-use");
            }
        }

        let client_session = self.client_connector.new_stream(&self.proxy_to).await;

        match client_session {
            Ok(client_session) => {
                return self.duplex(io, client_session).await;
            }
            Err(e) => {
                info!("Failed to create client session: {}", e);
                None
            }
        }
    }
}
// This works against the gRPC server: grpcurl -import-path ~/Documents/raft-implementation/raft/raft-grpc/proto -proto risdb.proto -insecure -d '{ "keys": ["Saudi Riyal"] }' "[::1]:50051" risdb.RisDb/Get

// TODO TODO TODO: This works as a proxy when using grpcurl and using kreya. Both ahve to operate in "insecure" mode though
// as the root authority for the certs can't be trusted (since its just self signed).
// It says downstream which to me implies the client to the proxy server is where the issue is but I
// can't confirm. But moral of the story it works for tcp and some for TLS but a raft client can't connect via TLS so
// that needs to be sorted
fn main() -> std::io::Result<()> {
    env_logger::init();

    let subject_alt_names = vec!["localhost".to_string()];
    let CertifiedKey { cert, key_pair } = generate_simple_self_signed(subject_alt_names).unwrap();

    let base_dir = format!("{}/certs/keys", env!("CARGO_MANIFEST_DIR"));
    println!("{}", base_dir);
    let cert_path = format!("{}/server.crt", base_dir);
    let priv_key_path = format!("{}/priv.key", base_dir);
    let pub_key_path = format!("{}/key.pem", base_dir);

    create_dir_all(base_dir)?;
    let mut cert_file = File::create(&cert_path)?;
    cert_file.write_all(cert.pem().as_bytes())?;
    let mut priv_key = File::create(&priv_key_path)?;
    priv_key.write_all(key_pair.serialize_pem().as_bytes())?;
    let mut pub_key = File::create(&pub_key_path)?;
    pub_key.write_all(key_pair.public_key_pem().as_bytes())?;

    let proxy_service = proxy_service_tcp(
        "[::1]:6141",    // listen
        "[::1]:50051",     // proxy to
        "", // SNI
    );
    let proxy_service_ssl = proxy_service_tls(
        "[::1]:6144",    // listen
        "[::1]:50051",     // proxy to
        "", // SNI
        &cert_path,
        &priv_key_path,
    );


    let opt = Some(Opt::from_args());
    let mut my_server = Server::new(opt).unwrap();
    my_server.bootstrap();

    let services: Vec<Box<dyn OtherService>> = vec![
        Box::new(proxy_service),
        Box::new(proxy_service_ssl),
    ];
    my_server.add_services(services);
    my_server.run_forever();

    Ok(())
}

fn proxy_service_tcp(
    addr: &str,
    proxy_addr: &str,
    proxy_sni: &str
) -> Service<ProxyApp> {
    let mut proxy_to = HttpPeer::new(proxy_addr, false, "".to_string());
    // set SNI to enable TLS
    proxy_to.sni = proxy_sni.into();
    Service::with_listeners(
        "Proxy Service TCP".to_string(),
        Listeners::tcp(addr),
        Arc::new(ProxyApp::new(proxy_to)),
    )
}

fn proxy_service_tls(
    addr: &str,
    proxy_addr: &str,
    proxy_sni: &str,
    cert_path: &str,
    key_path: &str,
) -> Service<ProxyApp> {
    let mut proxy_to = HttpPeer::new(proxy_addr, false, "".to_string());
    // set SNI to enable TLS
    proxy_to.sni = proxy_sni.into();
    proxy_to.get_mut_peer_options().unwrap().alpn = ALPN::H2;

    let mut tls_settings = TlsSettings::intermediate(cert_path, key_path).unwrap();
    tls_settings.enable_h2();

    let mut listeners = Listeners::new();
    listeners.add_tls_with_settings(addr, None, tls_settings);

    let service = Service::with_listeners(
        "Proxy Service TLS".to_string(),
        listeners,
        Arc::new(ProxyApp::new(proxy_to)),
    );
    service
}