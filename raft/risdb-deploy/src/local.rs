use std::process::Stdio;
use tokio::process::{Child, ChildStdout, Command};

#[derive(Clone)]
pub struct ServerArgs {
    /// Port to start risdb_hyper server on
    pub frontend_port: u16,
    /// Port to start raft server on
    pub raft_port: u16,
}

const HELLOWORLD_SERVER: &str = "helloworld-server";

pub fn start_local_test(server_args: Vec<ServerArgs>) -> Vec<(Child, ChildStdout)> {
    server_args.iter().for_each(|server_arg| {
        let _ = format!(
            "frontend_port: {:?}, raft_port: {:?}",
           server_arg.frontend_port, server_arg.raft_port
        );
    });

    let command_strings: Vec<String> = server_args
        .iter()
        .enumerate()
        .map(|(i, server_arg)| {
            let mut command_string = format!(
                "[::1]:{},[::1]:{}",
                server_arg.frontend_port, server_arg.raft_port,
            );

            let mut temp_server_args = server_args.clone();
            temp_server_args.remove(i);

            temp_server_args.iter().for_each(|peer_arg| {
                command_string.push_str(&format!(";http://[::1]:{}", peer_arg.raft_port))
            });

            command_string
        })
        .collect();

    command_strings
        .into_iter()
        .map(|command_str| {
            let mut child = Command::new(HELLOWORLD_SERVER.to_string())
                .arg(command_str)
                .stdout(Stdio::piped())
                .spawn()
                .expect("Failed to start helloworld-server");
            let stdout = child.stdout.take().expect("Failed to get handle on stdout");
            (child, stdout)
        })
        .collect()
}
