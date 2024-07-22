pub struct ServerArgs {
    /// Port to start risdb_hyper server on
    pub frontend_port: u16,
    /// Port to start raft server on
    pub raft_port: u16,
}

pub fn run_local_test(server_args: Vec<ServerArgs>) {
    server_args
        .iter()
        .for_each(|server_arg| { 
            let _ = format!("frontend_port: {:?}, raft_port: {:?}", server_arg.frontend_port, server_arg.raft_port);
        });
}
