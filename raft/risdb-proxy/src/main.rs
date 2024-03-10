use async_trait::async_trait;
use log::{error, info};
use pingora_load_balancing::{health_check, selection::RoundRobin, LoadBalancer};
use std::{net::SocketAddr, sync::{Arc, Mutex}, time::Duration};
use structopt::StructOpt;

use pingora::{listeners::TcpSocketOptions, services::{background::background_service, listening::Service as ListeningService}};
use pingora_core::server::configuration::Opt;
use pingora_core::server::Server;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_core::Result;
use pingora_proxy::{ProxyHttp, Session};

// global counter
static REQ_COUNTER: Mutex<usize> = Mutex::new(0);

pub struct MyProxy {
    // counter for the service
    beta_counter: Mutex<usize>, // AtomicUsize works too
}

pub struct MyCtx {
    beta_user: bool,
}

fn check_beta_user(req: &pingora_http::RequestHeader) -> bool {
    // some simple logic to check if user is beta
    req.headers.get("beta-flag").is_some()
}

#[async_trait]
impl ProxyHttp for MyProxy {
    type CTX = MyCtx;
    fn new_ctx(&self) -> Self::CTX {
        MyCtx { beta_user: false }
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        ctx.beta_user = check_beta_user(session.req_header());
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let mut req_counter = REQ_COUNTER.lock().unwrap();
        *req_counter += 1;

        let peer = if ctx.beta_user {
            let mut beta_count = self.beta_counter.lock().unwrap();
            *beta_count += 1;
            info!("I'm a beta user #{beta_count}");
            let addr = ("1.0.0.1", 443);

            Box::new(HttpPeer::new(addr, true, "one.one.one.one".to_string()))
        } else {
            info!("I'm an user #{req_counter}");
            let addr = ("blog.jtken.com", 443);

            Box::new(HttpPeer::new(addr, true, "jtken.com".to_string()))
        };

        Ok(peer)
    }
}

pub struct MyRoundRobinLB {
    lb: Arc<LoadBalancer<RoundRobin>>,
    tls: bool,
    sni: String
}

impl MyRoundRobinLB {
    fn new(addrs: Vec<&str>, tls: bool, sni: String) -> Self {
        let mut lb = LoadBalancer::try_from_iter(addrs).unwrap();

        // We add health check in the background so that the bad server is never selected.
        let hc = health_check::TcpHealthCheck::new();
        lb.set_health_check(hc);
        lb.health_check_frequency = Some(Duration::from_secs(1));

        let background = background_service("health check", lb);

        let lb: Arc<LoadBalancer<RoundRobin>> = background.task();

        MyRoundRobinLB {
            lb,
            tls,
            sni
        }
    }
}

#[async_trait]
impl ProxyHttp for MyRoundRobinLB {
    type CTX = ();
    fn new_ctx(&self) -> Self::CTX {}

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut ()) -> Result<Box<HttpPeer>> {
        info!("Starting upstream peer...");

        let upstream = self.lb
            .select(b"", 256) // hash doesn't matter
            .unwrap();

        info!("upstream peer is: {:?}", upstream);

        let peer = Box::new(HttpPeer::new(upstream, self.tls, self.sni.to_owned()));
        Ok(peer)
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut pingora_http::RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()> {
        upstream_request
            .insert_header("Host", "one.one.one.one")
            .unwrap();
        Ok(())
    }
}

pub struct LB(Arc<LoadBalancer<RoundRobin>>);

// RUST_LOG=INFO cargo run
// I changed it slightly so that it proxies to my website instead of to two different cloudflare websites
// curl 127.0.0.1:6190 -H "Host: jtken.com"
// curl 127.0.0.1:6190 -H "Host: one.one.one.one" -H "beta-flag: 1"
fn main() {
    env_logger::init();

    // read command line arguments
    let opt = Opt::from_args();
    let mut my_server = Server::new(Some(opt)).unwrap();
    my_server.bootstrap();

    let lb = MyRoundRobinLB::new(
        vec![
            "1.1.1.1:443", "1.0.0.1:443"
        ],
        true,
        "one.one.one.one".to_string()
    );

    let mut my_proxy = pingora_proxy::http_proxy_service(
        &my_server.configuration,
        lb
    );
    my_proxy.add_tcp_with_settings("[::1]:6190", TcpSocketOptions { ipv6_only: true });

    // TODO: Set up some metrics on this cause it seems neat
    let mut prometheus_service_http = ListeningService::prometheus_http_service();
    prometheus_service_http.add_tcp("127.0.0.1:6150");

    my_server.add_service(my_proxy);
    my_server.add_service(prometheus_service_http);
    my_server.run_forever();
}