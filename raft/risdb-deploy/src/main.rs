use superconsole::{Component, DrawMode, Line, Lines, SuperConsole, style::{Color, Stylize}};
use core::time;
use std::{io::{BufRead, BufReader}, process::Stdio, thread};
use clap::{Args, Parser, Subcommand};
use risdb_deploy::local::{run_local_test, ServerArgs};

/// Test RisDb locally or deploy RisDb remotely
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Test command (doesn't do anything)
    Test(TestArgs),
    /// Command for local testing
    Local(LocalArgs),
}

#[derive(Args)]
struct TestArgs {
    /// The pattern to look for
    #[arg(long)]
    pattern: String,
    /// The path to the file to read
    #[arg(long)]
    path: std::path::PathBuf,
}

#[derive(Args)]
struct LocalArgs {
    /// Number of servers to start
    #[arg(short, long, default_value_t = 3 )]
    num_servers: u8,
    /// Path to re-install the RisDb binary from. Only re-installs if this value is present 
    #[arg(long)]
    reinstall_path: Option<std::path::PathBuf>,
}

enum Setup<'a> {
    Start,
    Reinstall(&'a str),
    CompleteReinstall(&'a str),
    Done,
}

fn main() {
    let arguments = Cli::parse();
    arguments.command.execute_command();
}

impl Commands {
    fn execute_command(self) {
        match self {
            Commands::Test(test_args) => println!("pattern: {:?}, path: {:?}", test_args.pattern, test_args.path),
            Commands::Local(local_args) => local_args.execute(),
        }
    }
}

impl<'a> Setup<'a> {
    fn get_lines(&self) -> anyhow::Result<superconsole::Lines> {
        let line = match self {
            Setup::Start =>Line::from_iter([
                "Performing test setup".to_string().try_into()?
            ]),
            Setup::Reinstall(reinstall_path) => Line::from_iter([
                "Currently reinstalling helloworld-server from path ".to_string().try_into()?,
                reinstall_path.to_string().try_into()?,
            ]),
            Setup::CompleteReinstall(reinstall_path) => Line::from_iter([
                "Completed reinstall from path ".to_string().try_into()?,
                reinstall_path.to_string().try_into()?, 
            ]),
            Setup::Done => Line::from_iter([
                "ERROR: Test setup considered done before completing".to_string().with(Color::Red).try_into()?
            ])
        };
        Ok(Lines(vec![line]))
    }
}

impl<'a> Component for Setup<'a> {
    fn draw_unchecked(
        &self, 
        _dimensions: superconsole::Dimensions, 
        mode: superconsole::DrawMode
    ) -> anyhow::Result<superconsole::Lines> {
        match mode {
            DrawMode::Normal => {
                let lines = self.get_lines()?; 
                Ok(lines)
            },
            DrawMode::Final => {
                let line = Line::from_iter([
                    "Succesfully started test".to_string().try_into()?
                ]);
                Ok(Lines(vec![line]))
            }
        }
    }
}

const FIRST_FRONTEND_PORT: u16 = 5000;
const FIRST_RAFT_PORT: u16 = 50_000;

impl LocalArgs { 
    fn execute(self) { 
        let num_servers = self.num_servers as u16;

        let mut console = SuperConsole::new().expect("Failed to intialize SuperConsole...");

        console.render(&Setup::Start).unwrap();

        let frontend_ports: Vec<u16> = (FIRST_FRONTEND_PORT..FIRST_FRONTEND_PORT+num_servers).collect();
        let raft_ports : Vec<u16> = (FIRST_RAFT_PORT..FIRST_RAFT_PORT+num_servers).collect();
        let server_args = frontend_ports.into_iter()
            .zip(raft_ports.into_iter())
            .map(|(frontend_port, raft_port)| ServerArgs {
                frontend_port,
                raft_port,
            })
            .collect();

        if let Some(reinstall_path) = self.reinstall_path {
            let reinstall_path_str = reinstall_path.to_str().expect("Provided path could not be parsed");
                
            let pre_build_messages = vec![
                Line::from_iter([
                    "Starting to re-install helloworld-server".to_string().with(Color::Magenta).try_into().unwrap()
                ])
            ];
            console.emit(Lines(pre_build_messages));

            console.render(&Setup::Reinstall(reinstall_path_str)).unwrap();
            
            let stdout = std::process::Command::new(env!("CARGO"))
                .arg("install")
                .arg("--bin=helloworld-server")
                .arg(format!("--path={}", reinstall_path_str))
                .arg("--color=always")
                .stderr(Stdio::piped())
                .spawn()
                .expect("Failed to start install command...")
                .stderr
                .expect("Failed to get a handle on install stdout..");

            let mut reader = BufReader::new(stdout);

            let reinstall_successful = 'outer: loop {
                let mut lines = Vec::with_capacity(10);
                for _ in 1..=10 {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => break 'outer true,
                        Err(_) => {
                            println!("Failed to read install output. Failling...");
                            break 'outer false;
                        },
                        _ => (),
                    };
                    lines.push(line
                        .chars()
                        .into_iter()
                        // Superconsole doesn't allow non space whitespace so I just filter it out
                        // for now
                        .filter(|c| *c == ' ' || !c.is_whitespace())
                        .collect()
                    );
                }

                let messages = lines
                    .into_iter()
                    .map(|line: String|
                        Line::from_iter([
                            line.try_into().unwrap()
                        ])
                    )
                    .collect();
                console.emit(Lines(messages));

                console.render(&Setup::Reinstall(reinstall_path_str)).unwrap();
            };

            if reinstall_successful {
                console.render(&Setup::CompleteReinstall(reinstall_path_str)).unwrap();
            } else {
                println!("Failed to reinstall. Terminating test...");
                return;
            }
        }

        run_local_test(server_args);

        console.finalize(&Setup::Done).unwrap();
    }
}
