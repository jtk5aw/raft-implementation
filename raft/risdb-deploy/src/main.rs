use clap::{Args, Parser, Subcommand};
use risdb_deploy::local::{start_local_test, ServerArgs};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufRead, BufReader as StdBufReader},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use superconsole::{
    style::{Color, Stylize},
    Component, DrawMode, Line, Lines, SuperConsole,
};
use tokio::signal::ctrl_c;
use tokio::sync::mpsc;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdout},
};
use tokio_util::sync::CancellationToken;

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
    path: PathBuf,
}

#[derive(Args)]
struct LocalArgs {
    /// Number of servers to start
    #[arg(short, long, default_value_t = 3)]
    num_servers: u8,
    /// Path to re-install the RisDb binary from. Only re-installs if this value is present
    #[arg(long)]
    reinstall_path: Option<PathBuf>,
}

enum Setup<'a> {
    Start,
    Reinstall(&'a str),
    CompleteReinstall(&'a str),
    Done,
}

enum InProgress<'a> {
    NoLogs,
    Logs(&'a Duration, &'a BTreeMap<String, Box<VecDeque<String>>>),
}

#[tokio::main]
async fn main() {
    let arguments = Cli::parse();
    arguments.command.execute_command().await;
}

impl Commands {
    async fn execute_command(self) {
        match self {
            Commands::Test(test_args) => println!(
                "pattern: {:?}, path: {:?}",
                test_args.pattern, test_args.path
            ),
            Commands::Local(local_args) => {
                let cancellation_token = CancellationToken::new();

                let program = tokio::spawn(local_args.execute(cancellation_token.clone()));

                let _ = ctrl_c().await;
                cancellation_token.cancel();

                let _ = program.await;
            }
        }
    }
}

impl<'a> Setup<'a> {
    fn get_lines(&self) -> anyhow::Result<superconsole::Lines> {
        let line = match self {
            Setup::Start => Line::from_iter(["Performing test setup".to_string().try_into()?]),
            Setup::Reinstall(reinstall_path) => Line::from_iter([
                "Currently reinstalling helloworld-server from path "
                    .to_string()
                    .try_into()?,
                reinstall_path.to_string().try_into()?,
            ]),
            Setup::CompleteReinstall(reinstall_path) => Line::from_iter([
                "Completed reinstall from path ".to_string().try_into()?,
                reinstall_path.to_string().try_into()?,
            ]),
            Setup::Done => Line::from_iter(["ERROR: Test setup considered done before completing"
                .to_string()
                .with(Color::Red)
                .try_into()?]),
        };
        Ok(Lines(vec![line]))
    }
}

impl<'a> Component for Setup<'a> {
    fn draw_unchecked(
        &self,
        _dimensions: superconsole::Dimensions,
        mode: superconsole::DrawMode,
    ) -> anyhow::Result<superconsole::Lines> {
        match mode {
            DrawMode::Normal => {
                let lines = self.get_lines()?;
                Ok(lines)
            }
            DrawMode::Final => {
                let line =
                    Line::from_iter(["Succesfully completed test".to_string().try_into()?]);
                Ok(Lines(vec![line]))
            }
        }
    }
}

impl<'a> Component for InProgress<'a> {
    fn draw_unchecked(
        &self,
        _dimensions: superconsole::Dimensions,
        _mode: DrawMode,
    ) -> anyhow::Result<Lines> {
        match self {
            Self::NoLogs => {
                let line = Line::from_iter(["This is a second test".to_string().try_into()?]);
                Ok(Lines(vec![line]))
            }
            Self::Logs(elapsed, log_tracker) => {
                let mut lines: Vec<Line> =
                    Vec::with_capacity(log_tracker.len() * (NUM_NODE_LOGS + 1) + 1);

                for (id, logs) in log_tracker.iter() {
                    let leading_line = Line::from_iter([format!("Logs for {}", id)
                        .with(Color::Cyan)
                        .try_into()?]);
                    lines.push(leading_line);

                    let logs = *logs.clone();
                    let mut log_lines = filter_bad_characters(logs);
                    lines.append(&mut log_lines);
                }

                let time_elapsed_line =
                    Line::from_iter([format!("Test run for {:?}", elapsed).try_into()?]);
                lines.push(time_elapsed_line);

                Ok(Lines(lines))
            }
        }
    }
}

const FIRST_FRONTEND_PORT: u16 = 5000;
const FIRST_RAFT_PORT: u16 = 50_000;
const NUM_NODE_LOGS: usize = 10;

impl LocalArgs {
    async fn execute(self, cancellation_token: CancellationToken) {
        let num_servers = self.num_servers as u16;

        let mut console = SuperConsole::new().expect("Failed to intialize SuperConsole...");

        console.render(&Setup::Start).unwrap();

        let frontend_ports: Vec<u16> =
            (FIRST_FRONTEND_PORT..FIRST_FRONTEND_PORT + num_servers).collect();
        let raft_ports: Vec<u16> = (FIRST_RAFT_PORT..FIRST_RAFT_PORT + num_servers).collect();
        let server_args = frontend_ports
            .into_iter()
            .zip(raft_ports.into_iter())
            .map(|(frontend_port, raft_port)| ServerArgs {
                frontend_port,
                raft_port,
            })
            .collect();

        if let Some(reinstall_path) = self.reinstall_path {
            let reinstall_path_str = reinstall_path
                .to_str()
                .expect("Provided path could not be parsed");
            let reinstall_successful = reinstall(&mut console, reinstall_path_str);

            if reinstall_successful {
                console
                    .render(&Setup::CompleteReinstall(reinstall_path_str))
                    .unwrap();
            } else {
                println!("Failed to reinstall. Terminating test...");
                return;
            }
        }

        console.render(&InProgress::NoLogs).unwrap();

        let (children, stdouts): (Vec<Child>, Vec<ChildStdout>) =
            start_local_test(server_args).into_iter().unzip();

        children.into_iter().for_each(|mut child| {
            let cloned_cancellation_token = cancellation_token.clone();
            tokio::spawn(async move {
                let _ = cloned_cancellation_token.cancelled().await;
                // Once this is cancelled kill this child process
                let _ = child.kill().await;
            });
        });

        let (sender, mut receiver) = mpsc::channel(32);

        let mut log_tracker: BTreeMap<String, Box<VecDeque<String>>> = BTreeMap::new();
        stdouts.into_iter().enumerate().for_each(|(i, stdout)| {
            let sender_clone = sender.clone();
            log_tracker.insert(i.to_string(), Box::new(VecDeque::with_capacity(10)));

            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Err(err) => return Err(err),
                        Ok(0) => {
                            let close_message = Message::Close(CloseMessage { sender_id: i });
                            let _ = sender_clone.send(close_message).await;

                            return Ok(());
                        }
                        Ok(_) => {
                            let log_message = Message::Log(LogMessage {
                                sender_id: i,
                                message: line.to_owned(),
                            });
                            let _ = sender_clone.send(log_message).await;
                        }
                    }
                }
            });
        });

        let start = std::time::SystemTime::now();
        let mut num_completed = 0;
        while let Some(message) = receiver.recv().await {
            match message {
                Message::Log(log_message) => {
                    let sender_id = log_message.sender_id.to_string();
                    log_tracker.get_mut(&sender_id).map(|log_deque| {
                        log_deque.push_back(log_message.message);
                        if log_deque.len() > NUM_NODE_LOGS {
                            log_deque.pop_front();
                        }
                    });
                }
                Message::Close(close_message) => {
                    let line = Line::from_iter(vec![format!(
                        "Received close from {}",
                        close_message.sender_id
                    )
                    .try_into()
                    .unwrap()]);
                    console.emit(Lines(vec![line]));
                    num_completed = num_completed + 1;
                }
            }
            let elapsed = start.elapsed().unwrap_or_else(|_| Duration::new(0, 0));
            console
                .render(&InProgress::Logs(&elapsed, &log_tracker))
                .unwrap();
            if num_completed == num_servers {
                break;
            }
        }

        // TODO TODO TODO: Write all logs to a file and on finalize write out the file name where
        // logs were written

        console.finalize(&Setup::Done).unwrap();
    }
}

fn reinstall(console: &mut SuperConsole, reinstall_path_str: &str) -> bool {
    let pre_build_messages = vec![Line::from_iter([
        "Starting to re-install helloworld-server"
            .to_string()
            .with(Color::Magenta)
            .try_into()
            .unwrap(),
    ])];
    console.emit(Lines(pre_build_messages));

    console
        .render(&Setup::Reinstall(reinstall_path_str))
        .unwrap();

    let stderr = std::process::Command::new(env!("CARGO"))
        .arg("install")
        .arg("--bin=helloworld-server")
        .arg(format!("--path={}", reinstall_path_str))
        .arg("--color=always")
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start install command...")
        .stderr
        .expect("Failed to get a handle on install stderr..");

    let mut reader = StdBufReader::new(stderr);

    let reinstall_successful = 'outer: loop {
        let mut lines = Vec::with_capacity(10);
        for _ in 1..=10 {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break 'outer true,
                Err(_) => {
                    println!("Failed to read install output. Failling...");
                    break 'outer false;
                }
                _ => (),
            };

            lines.push(line);
        }

        let messages = filter_bad_characters(lines);
        console.emit(Lines(messages));

        console
            .render(&Setup::Reinstall(reinstall_path_str))
            .unwrap();
    };

    reinstall_successful
}

enum Message {
    Log(LogMessage),
    Close(CloseMessage),
}

struct LogMessage {
    sender_id: usize,
    message: String,
}

struct CloseMessage {
    sender_id: usize,
}

fn filter_bad_characters<I>(lines: I) -> Vec<Line>
where
    I: IntoIterator<Item = String>,
{
    lines
        .into_iter()
        .map(|str| {
            str.chars()
                .into_iter()
                .filter(|c| *c == ' ' || !c.is_whitespace())
                .collect::<String>()
        })
        .map(|line| Line::from_iter([line.try_into().unwrap()]))
        .collect()
}
