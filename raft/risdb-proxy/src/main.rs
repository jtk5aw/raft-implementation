use async_trait::async_trait;
use pingora::{ Error, listeners::TlsSettings, server::{configuration::Opt, Server}, services::{Service as OtherService}, upstreams::peer::HttpPeer};
use structopt::StructOpt;
use std::sync::Arc;
use std::path::{Path, PathBuf};
use pingora_core::protocols::ALPN;
use pingora_core::upstreams::peer::Peer;
use pingora_load_balancing::LoadBalancer;
use pingora_load_balancing::selection::RoundRobin;
use pingora_proxy::{http_proxy_service, ProxyHttp, Session};

pub struct LB(Arc<LoadBalancer<RoundRobin>>);

#[async_trait]
impl ProxyHttp for LB {

    /// For this small example, we don't need context storage
    type CTX = ();
    fn new_ctx(&self) -> () {
        ()
    }

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut ()) -> Result<Box<HttpPeer>, Box<Error>> {
        let upstream = self.0
            .select(b"", 256) // hash doesn't matter for round robin
            .unwrap();

        println!("upstream peer is: {upstream:?}");

        let mut peer = HttpPeer::new(upstream, true, "".to_string());
        match peer.get_mut_peer_options() {
            Some(peer_options) => peer_options.alpn = ALPN::H2,
            None => {
                println!("Failed to set ALPN as H2");
            }
        };
        let peer = Box::new(peer);
        Ok(peer)
    }
}

const CERTS: &str = "certs";
const RISDB_PEM: &str = "risdb.pem";
const RISDB_EC: &str = "risdb.ec";

fn main() -> std::io::Result<()> {
    env_logger::init();

    // Get the Cert and Priv key paths
    let base_dir = get_workspace_base_dir();
    let cert_path = &base_dir
        .join(CERTS)
        .join(RISDB_PEM);
    let cert_path = cert_path.to_str().unwrap();
    let key_path = &base_dir
        .join(CERTS)
        .join(RISDB_EC);
    let key_path = key_path.to_str().unwrap();

    // Start the server
    let opt = Some(Opt::from_args());
    let mut my_server = Server::new(opt).unwrap();
    my_server.bootstrap();

    // Start remaining services
    let upstreams =
        LoadBalancer::try_from_iter(["[::1]:3000", "[::1]:3001"]).unwrap();

    let mut lb = http_proxy_service(&my_server.configuration, LB(Arc::new(upstreams)));
    let mut tls_settings =
        TlsSettings::intermediate(cert_path, key_path).unwrap();
    tls_settings.enable_h2();
    lb.add_tls_with_settings("0.0.0.0:6189", None, tls_settings);


    let services: Vec<Box<dyn OtherService>> = vec![
        Box::new(lb),
    ];
    my_server.add_services(services);
    my_server.run_forever();

    Ok(())
}

pub fn get_workspace_base_dir() -> PathBuf {
    let output = std::process::Command::new(env!("CARGO"))
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format=plain")
        .output()
        .unwrap()
        .stdout;
    let cargo_path = Path::new(std::str::from_utf8(&output).unwrap().trim());
    cargo_path.parent().unwrap().to_path_buf()
}