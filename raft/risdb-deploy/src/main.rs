use ansi_to_tui::IntoText;
use clap::{Args, Parser, Subcommand};
use color_eyre::eyre::{bail, Context, Report};
use crossterm::{
    event::{self, poll, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize as _},
    text::{Line, Span},
    widgets::{
        block::{Position, Title}, Block, BorderType, Borders, Paragraph, Widget
    },
    Frame, Terminal,
};
use std::{
    collections::{BTreeMap, VecDeque}, error::Error, fmt::Display, io::{stdout, Stdout}, path::PathBuf, process::Stdio, sync::mpsc::{self, Receiver, Sender}, time::Duration
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::runtime::Runtime;
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

#[derive(Args, Clone)]
struct LocalArgs {
    /// Number of servers to start
    #[arg(short, long, default_value_t = 3)]
    num_servers: u8,
    /// Path to re-install the RisDb binary from. Only re-installs if this value is present
    #[arg(long)]
    reinstall_path: Option<PathBuf>,
}

// enum Setup<'a> {
//     Start,
//     Reinstall(&'a str),
//     CompleteReinstall(&'a str),
//     Done,
// }
//
// enum InProgress<'a> {
//     NoLogs,
//     Logs(&'a Duration, &'a BTreeMap<String, Box<VecDeque<String>>>),
// }

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let arguments = Cli::parse();
    arguments.command.execute_command()
}

pub fn tui_init() -> std::io::Result<Tui> {
    execute!(stdout(), EnterAlternateScreen)?;
    enable_raw_mode()?;
    set_panic_hook();
    Terminal::new(CrosstermBackend::new(stdout()))
}

fn set_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore();
        hook(panic_info);
    }));
}

pub fn restore() -> std::io::Result<()> {
    execute!(stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

impl Commands {
    fn execute_command(self) -> color_eyre::Result<()> {
        match self {
            Commands::Test(test_args) => {
                println!(
                    "pattern: {:?}, path: {:?}",
                    test_args.pattern, test_args.path
                );
                Ok(())
            }
            Commands::Local(local_args) => {
                let (sender, receiver) = mpsc::channel();
                let mut local_app_state =
                    LocalAppState::new(local_args.num_servers as u16, sender.clone());
                let mut local_app = LocalApp::new(receiver, Runtime::new()?);
                let mut terminal = tui_init()?;

                match local_args.reinstall_path {
                    Some(path) => {
                        sender.send(LocalAppMessage::StartInstall(StartInstallMessage { path }))?
                    }
                    None => sender.send(LocalAppMessage::StartServers)?,
                };

                let run_result = local_app.run(&mut local_app_state, &mut terminal)
                    .map_err(|report| {
                        let root_cause = report.root_cause().downcast_ref::<NoBacktraceError>();
                        match root_cause {
                            Some(no_backtrace_error) => 
                                Report::msg(no_backtrace_error.msg.to_owned()),
                            None => report
                        }
                    });
                if let Err(err) = restore() {
                    eprintln!(
                        "failed to restore terminal. Run `reset` or exit your terminal to recover: {}",
                        err
                    );
                };

                //let _ = ctrl_c().await;
                //cancellation_token.cancel();

                run_result
            }
        }
    }
}

impl LocalAppState {
    fn new(num_servers: u16, sender: Sender<LocalAppMessage>) -> Self {
        Self {
            num_servers,
            sender,
            cancellation_token: CancellationToken::new(),
            exit: false,
            kind: LocalAppStateKind::Start,
        }
    }
}

impl LocalApp {
    fn new(receiver: Receiver<LocalAppMessage>, runtime: Runtime) -> Self {
        Self { receiver, runtime }
    }
}

// Reason for using the std::mpsc:channel is here
// https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html#communicating-between-sync-and-async-code
// Since the receiver is in sync code the recommendation is to use the std receiver
struct LocalApp {
    // Tokio runtime used to spawn tasks. These tasks will track state asynchronously that is
    // communicated back for the terminal to use
    runtime: Runtime,
    // Receiver used to receive state updates that will update contents of the terminal
    // This will always exist in sync code
    receiver: Receiver<LocalAppMessage>,
}

struct LocalAppState {
    // Number of servers
    num_servers: u16,
    // Sender used to communicate state updates to the terminal
    // This will (almost?) always exist in async code
    sender: Sender<LocalAppMessage>,
    // Token used to tell all running servers to shut down
    cancellation_token: CancellationToken,
    // Boolean used to indicate the terminal should be closed
    exit: bool,
    // Kind specific data
    kind: LocalAppStateKind,
}

enum LocalAppStateKind {
    Start,
    Install(InstallState),
    InProgress(InProgressState),
}

struct InstallState {
    complete: bool,
    end_line: Option<usize>,
    lines: Vec<String>,
}

struct InProgressState {
    logs: BTreeMap<String, Box<RecentLogs>>,
}

#[derive(Debug)]
struct NoBacktraceError {
    msg: String,
}

impl NoBacktraceError {
    fn msg(msg: String) -> Self {
        Self { msg }
    }
}

impl Display for NoBacktraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg.as_str())
    }
}

impl Error for NoBacktraceError {}

impl LocalAppState {
    fn start_exit(&mut self) {
        // TODO TODO TODO: Instead set this to set the cancellation token which will eventually
        // lead to exit being set to true
        self.exit = true;
        self.cancellation_token.cancel();
    }

    fn decrease_scroll(&mut self) {
        match &mut self.kind {
            LocalAppStateKind::Install(install_app_state) => {
                let new_offset_starting_line = match install_app_state.end_line {
                    Some(val) => {
                        if val != 0 {
                            val - 1
                        } else {
                            val
                        }
                    }
                    None => install_app_state.lines.len(),
                };
                install_app_state.end_line = Some(new_offset_starting_line);
            }
            _ => {}
        }
    }

    fn increase_scroll(&mut self) {
        match &mut self.kind {
            LocalAppStateKind::Install(install_app_state) => {
                let new_offset_starting_line = match install_app_state.end_line {
                    Some(val) => {
                        if val != 0 {
                            val + 1
                        } else {
                            val
                        }
                    }
                    None => install_app_state.lines.len(),
                };
                install_app_state.end_line = Some(new_offset_starting_line);
            }
            _ => {}
        }
    }

    fn try_continue(&mut self) {
        match &mut self.kind {
            LocalAppStateKind::Install(install_app_state) => {
                if install_app_state.complete {
                    self.sender.send(LocalAppMessage::StartServers).expect("request to start test servers failed");
                }
            },
            _ => {},
        }
    }

    fn try_stop(&self) -> color_eyre::Result<()> {
        match &self.kind {
            LocalAppStateKind::Install(install_app_state) => {
                if install_app_state.complete {
                    return Err(Report::new(NoBacktraceError::msg("user requested to stop".to_string())));
                }
            },
            _ => {},
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) -> color_eyre::Result<()> {
        match key_event.code {
            KeyCode::Char('q') => self.start_exit(),
            KeyCode::Char('k') => self.decrease_scroll(),
            KeyCode::Char('j') => self.increase_scroll(),
            KeyCode::Char('y') => self.try_continue(),
            KeyCode::Char('n') => self.try_stop()?,
            _ => {}
        }
        Ok(())
    }

    fn handle_message(
        &mut self,
        runtime: &Runtime,
        message: LocalAppMessage,
    ) -> color_eyre::Result<()> {
        match message {
            LocalAppMessage::StartInstall(start_install_message) => match &mut self.kind {
                LocalAppStateKind::Start => {
                    self.kind = LocalAppStateKind::Install(InstallState {
                        complete: false,
                        lines: Vec::new(),
                        end_line: None,
                    });

                    let start_install_sender = self.sender.clone();
                    let start_install_token = self.cancellation_token.clone();
                    runtime.spawn(LocalAppState::reinstall(
                            start_install_sender,
                            start_install_token,
                            start_install_message
                            .path
                            .to_str()
                            .expect("install path is not a valid string")
                            .to_string(),
                    ));
                    Ok(())
                }
                _ => bail!("local deploy did not initialize properly"),
            },
            // TODO TODO TODO: Spawn the tasks using the method used by the superconsole stuff
            // MAKE SURE THEY'RE WIRED UP WITH THE CANCELLATION TOKEN AS THAT'S ALREADY SET TO
            // CANCEL
            LocalAppMessage::StartServers => todo!("Can't handle starting server yet"),
            LocalAppMessage::Install(install_message) => match &mut self.kind {
                LocalAppStateKind::Install(install_state) => {
                    install_state.lines.extend(install_message.lines);
                    Ok(())
                }
                _ => bail!("install did not complete properly"),
            },
            LocalAppMessage::InstallComplete => match &mut self.kind {
                LocalAppStateKind::Install(install_state) => {
                    install_state.complete = true;
                    Ok(())
                }
                _ => bail!("install did not complete properly"),

            },
            // TODO TODO TODO: Almost certainly aren't gonna wrap this up today so can start here
            // and above handling startServers
            LocalAppMessage::ServerLog(_) => todo!("Can't handle server logs yet"),
        }
    }

    async fn reinstall(
        sender: Sender<LocalAppMessage>,
        cancellation_token: CancellationToken,
        reinstall_path_str: String,
    ) {
        sender
            .send(LocalAppMessage::Install(InstallMessage {
                lines: vec!["Starting to re-install helloworld-server\n\n".into()],
            }))
            .expect("failed to start installing helloworld-server");

        let stderr = tokio::process::Command::new(env!("CARGO"))
            .arg("install")
            .arg("--bin=helloworld-server")
            .arg(format!("--path={}", reinstall_path_str))
            .arg("--color=always")
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start install command...")
            .stderr
            .expect("Failed to get a handle on install stderr..");

        let mut reader = BufReader::new(stderr);

        'outer: loop {
            // TODO: This might not be necessary/do anything? Putting it here just in case
            if cancellation_token.is_cancelled() {
                break;
            }
            let mut lines = Vec::with_capacity(10);
            for _ in 1..=10 {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => break 'outer,
                    Err(_) => {
                        println!("Failed to read install output. Failling...");
                        break 'outer;
                    }
                    _ => (),
                };
                lines.push(line);
            }

            sender
                .send(LocalAppMessage::Install(InstallMessage { lines }))
                .expect("failed to update install progress");
        }

        sender.send(LocalAppMessage::InstallComplete).expect("failed to send install complete");
    }
}

enum LocalAppMessage {
    StartInstall(StartInstallMessage),
    Install(InstallMessage),
    InstallComplete,
    StartServers,
    ServerLog(ServerLogMessage),
}

struct StartInstallMessage {
    path: PathBuf,
}

struct InstallMessage {
    lines: Vec<String>,
}
struct ServerLogMessage {
    sender_id: usize,
    message: String,
}

#[derive(Debug, Clone)]
struct RecentLogs {
    log_vec: VecDeque<String>,
}

impl Default for RecentLogs {
    fn default() -> Self {
        Self {
            log_vec: VecDeque::with_capacity(10),
        }
    }
}

impl LocalApp {
    pub fn run(
        &mut self,
        local_app_state: &mut LocalAppState,
        terminal: &mut Tui,
    ) -> color_eyre::Result<()> {
        while !local_app_state.exit {
            terminal.draw(|frame| match &local_app_state.kind {
                LocalAppStateKind::Install(install_state) => {
                    self.render_install_frame(frame, install_state)
                }
                LocalAppStateKind::InProgress(recent_logs) => {
                    self.render_logs_frame(frame, local_app_state.num_servers, &recent_logs)
                }
                _ => {}
            })
            .wrap_err("failed to draw")?;

            self.handle_events(local_app_state)
                .wrap_err("failed to handle events")?;
            }
        Ok(())
    }

    fn render_install_frame(&self, frame: &mut Frame, install_state: &InstallState) {
        let text_render_area = if install_state.complete {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints(vec![Constraint::Percentage(95), Constraint::Percentage(5)])
                .split(frame.area());

            let bottom_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(vec![Constraint::Percentage(20), Constraint::Percentage(80)])
                .split(layout[1]);

            let continue_text = Line::from(vec![
                Span::styled("Continue? y/n", Style::new().green().italic().bold()),
            ]);
            let continue_paragraph = Paragraph::new(continue_text)
                .centered()
                .block(
                    Block::new()
                    .borders(Borders::all())
                    .border_type(BorderType::Thick)
                    .style(Style::default().green())
                );

            frame.render_widget(continue_paragraph, bottom_layout[0]);

            layout[0]
        } else {
            frame.area()
        };

        let height = text_render_area.height as usize;
        let line_len = install_state.lines.len();
        let end_index = install_state
            .end_line
            .unwrap_or_else(|| install_state.lines.len());

        let lines_to_render = if line_len > end_index && end_index > height {
            &install_state.lines[(end_index - height)..end_index]
        } else if line_len > end_index {
            &install_state.lines[0..end_index]
        } else if end_index > height {
            &install_state.lines[(end_index - height)..]
        } else {
            &install_state.lines[..]
        };

        let joined_lines = lines_to_render.join("");
        let lines_as_text =
            IntoText::to_text(&joined_lines).expect("failed to convert cargo install output");
        let paragraph = Paragraph::new(lines_as_text);

        frame.render_widget(paragraph, text_render_area);
    }

    fn render_logs_frame(&self, frame: &mut Frame, num_servers: u16, logs_state: &InProgressState) {
        let outer_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Percentage(5), Constraint::Percentage(95)])
            .split(frame.area());

        let title = Title::from(Line::from("Risdb Local").bold().blue().on_white());

        let title_block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default())
            .title(title.alignment(Alignment::Center).position(Position::Top))
            .title_style(Style::new().blue().on_white().bold().italic());

        frame.render_widget(title_block, outer_layout[0]);

        let log_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(
                std::iter::repeat(num_servers)
                    .take(num_servers.into())
                    .map(|val| Constraint::Ratio(1, val.into())),
            )
            .split(outer_layout[1]);

        for (i, recent_logs) in logs_state.logs.values().enumerate() {
            frame.render_widget(*recent_logs.clone(), log_layout[i]);
        }
    }

    fn handle_events(&mut self, local_app_state: &mut LocalAppState) -> color_eyre::Result<()> {
        // Read for keyboard events
        // Polling guarantees that an event will be there
        if poll(Duration::from_millis(1))? {
            match event::read()? {
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => local_app_state
                    .handle_key_event(key_event)
                    .wrap_err_with(|| format!("handling key event failed:\n{key_event:#?}")),
                _ => Ok(()),
            }?;
        }

        // Read for message events
        match self.receiver.recv_timeout(Duration::from_millis(1)) {
            Ok(message) => local_app_state
                .handle_message(&self.runtime, message)
                .wrap_err("handling recv failed"),
            // Timed out, do nothing
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
            // Also do nothing but TODO: Determine if should exit in this case
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(()),
        }?;

        Ok(())
    }
}

impl Widget for RecentLogs {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        // TODO: This almost certainly defeats the purpose of using VecDeque at all but :shrug: for
        // now
        let joined_lines = self.log_vec.into_iter().collect::<Vec<String>>().join("\n");
        let lines_as_text = IntoText::to_text(&joined_lines).expect("failed to convert server logs");
        Paragraph::new(lines_as_text).render(area, buf);
    }
}

// impl<'a> Setup<'a> {
//     fn get_lines(&self) -> anyhow::Result<superconsole::Lines> {
//         let line = match self {
//             Setup::Start => {
//                 superconsole::Line::from_iter(["Performing test setup".to_string().try_into()?])
//             }
//             Setup::Reinstall(reinstall_path) => superconsole::Line::from_iter([
//                 "Currently reinstalling helloworld-server from path "
//                     .to_string()
//                     .try_into()?,
//                 reinstall_path.to_string().try_into()?,
//             ]),
//             Setup::CompleteReinstall(reinstall_path) => superconsole::Line::from_iter([
//                 "Completed reinstall from path ".to_string().try_into()?,
//                 reinstall_path.to_string().try_into()?,
//             ]),
//             Setup::Done => superconsole::Line::from_iter([
//                 "ERROR: Test setup considered done before completing"
//                     .to_string()
//                     .with(Color::Red)
//                     .try_into()?,
//             ]),
//         };
//         Ok(superconsole::Lines(vec![line]))
//     }
// }
//
// impl<'a> Component for Setup<'a> {
//     fn draw_unchecked(
//         &self,
//         _dimensions: superconsole::Dimensions,
//         mode: superconsole::DrawMode,
//     ) -> anyhow::Result<superconsole::Lines> {
//         match mode {
//             DrawMode::Normal => {
//                 let lines = self.get_lines()?;
//                 Ok(lines)
//             }
//             DrawMode::Final => {
//                 let line = superconsole::Line::from_iter(["Succesfully completed test"
//                     .to_string()
//                     .try_into()?]);
//                 Ok(superconsole::Lines(vec![line]))
//             }
//         }
//     }
// }
//
// impl<'a> InProgress<'a> {
//     fn get_lines(&self, dimensions: Dimensions) -> anyhow::Result<superconsole::Lines> {
//         match self {
//             Self::NoLogs => {
//                 let line = superconsole::Line::from_iter(["This is a second test"
//                     .to_string()
//                     .try_into()?]);
//                 Ok(superconsole::Lines(vec![line]))
//             }
//             Self::Logs(elapsed, log_tracker) => {
//                 let mut lines: Vec<superconsole::Line> =
//                     Vec::with_capacity(log_tracker.len() * (NUM_NODE_LOGS + 1) + 1);
//
//                 // TODO TODO TODO: Rather than emitting these all in  one component. Draw each
//                 // individual set of logs as its own component. This will lead to less bouncing
//                 // around and make the logs easier to read. The tough part will be that
//                 // superconsole claims to support rendering  sub components but it really doesn't
//
//                 for (id, logs) in log_tracker.iter() {
//                     let leading_line = superconsole::Line::from_iter([format!("Logs for {}", id)
//                         .with(Color::Cyan)
//                         .try_into()?]);
//                     lines.push(leading_line);
//
//                     // TODO TODO TODO: Make it so graphemes are cut off as full words. Right now
//                     // words cross line boundaries which makes it tough
//                     let logs = *logs.clone();
//                     let mut log_lines = logs
//                         .iter()
//                         .flat_map(|line| {
//                             let mut graphemes = line.as_str().graphemes(true);
//                             std::iter::from_fn(move || {
//                                 let chunk: Vec<&str> =
//                                     graphemes.by_ref().take(dimensions.width).collect();
//                                 if chunk.is_empty() {
//                                     return None;
//                                 }
//                                 Some(chunk.concat())
//                             })
//                         })
//                         .map(|line| superconsole::Line::sanitized(&line))
//                         .collect();
//                     lines.append(&mut log_lines);
//                 }
//
//                 let time_elapsed_line =
//                     superconsole::Line::from_iter([
//                         format!("Test run for {:?}", elapsed).try_into()?
//                     ]);
//                 lines.push(time_elapsed_line);
//
//                 Ok(superconsole::Lines(lines))
//             }
//         }
//     }
// }
//
// impl<'a> Component for InProgress<'a> {
//     fn draw_unchecked(
//         &self,
//         dimensions: superconsole::Dimensions,
//         mode: DrawMode,
//     ) -> anyhow::Result<superconsole::Lines> {
//         match mode {
//             DrawMode::Normal => {
//                 let lines = self.get_lines(dimensions)?;
//                 Ok(lines)
//             }
//             DrawMode::Final => {
//                 let lines = self.get_lines(dimensions)?;
//                 Ok(lines)
//             } //DrawMode::Final => Ok(superconsole::Lines(vec![])),
//         }
//     }
// }
//
// const FIRST_FRONTEND_PORT: u16 = 5000;
// const FIRST_RAFT_PORT: u16 = 50_000;
// const NUM_NODE_LOGS: usize = 10;
//
// impl LocalArgs {
//     async fn execute(self, cancellation_token: CancellationToken) {
//         let num_servers = self.num_servers as u16;
//
//         let mut console = SuperConsole::new().expect("Failed to intialize SuperConsole...");
//
//         console.render(&Setup::Start).unwrap();
//
//         let frontend_ports: Vec<u16> =
//             (FIRST_FRONTEND_PORT..FIRST_FRONTEND_PORT + num_servers).collect();
//         let raft_ports: Vec<u16> = (FIRST_RAFT_PORT..FIRST_RAFT_PORT + num_servers).collect();
//         let server_args = frontend_ports
//             .into_iter()
//             .zip(raft_ports.into_iter())
//             .map(|(frontend_port, raft_port)| ServerArgs {
//                 frontend_port,
//                 raft_port,
//             })
//             .collect();
//
//         if let Some(reinstall_path) = self.reinstall_path {
//             let reinstall_path_str = reinstall_path
//                 .to_str()
//                 .expect("Provided path could not be parsed");
//             let reinstall_successful = reinstall(&mut console, reinstall_path_str);
//
//             if reinstall_successful {
//                 console
//                     .render(&Setup::CompleteReinstall(reinstall_path_str))
//                     .unwrap();
//             } else {
//                 println!("Failed to reinstall. Terminating test...");
//                 return;
//             }
//         }
//
//         console.render(&InProgress::NoLogs).unwrap();
//
//         let (children, stdouts): (Vec<Child>, Vec<ChildStdout>) =
//             start_local_test(server_args).into_iter().unzip();
//
//         children.into_iter().for_each(|mut child| {
//             let cloned_cancellation_token = cancellation_token.clone();
//             tokio::spawn(async move {
//                 let _ = cloned_cancellation_token.cancelled().await;
//                 // Once this is cancelled kill this child process
//                 let _ = child.kill().await;
//             });
//         });
//
//         let (sender, mut receiver) = mpsc::channel();
//
//         let mut log_tracker: BTreeMap<String, Box<VecDeque<String>>> = BTreeMap::new();
//         stdouts.into_iter().enumerate().for_each(|(i, stdout)| {
//             let sender_clone = sender.clone();
//             log_tracker.insert(i.to_string(), Box::new(VecDeque::with_capacity(10)));
//
//             tokio::spawn(async move {
//                 let mut reader = BufReader::new(stdout);
//                 let mut line = String::new();
//                 loop {
//                     line.clear();
//                     match reader.read_line(&mut line).await {
//                         Err(err) => return Err(err),
//                         Ok(0) => {
//                             let close_message = Message::Close(CloseMessage { sender_id: i });
//                             let _ = sender_clone.send(close_message).await;
//
//                             return Ok(());
//                         }
//                         Ok(_) => {
//                             let log_message = Message::Log(LogMessage {
//                                 sender_id: i,
//                                 message: line.to_owned(),
//                             });
//                             let _ = sender_clone.send(log_message).await;
//                         }
//                     }
//                 }
//             });
//         });
//
//         let start = std::time::SystemTime::now();
//         let mut num_completed = 0;
//         while let Some(message) = receiver.recv().await {
//             match message {
//                 Message::Log(log_message) => {
//                     let sender_id = log_message.sender_id.to_string();
//                     log_tracker.get_mut(&sender_id).map(|log_deque| {
//                         log_deque.push_back(log_message.message);
//                         if log_deque.len() > NUM_NODE_LOGS {
//                             log_deque.pop_front();
//                         }
//                     });
//                 }
//                 Message::Close(close_message) => {
//                     let line = superconsole::Line::from_iter(vec![format!(
//                         "Received close from {}",
//                         close_message.sender_id
//                     )
//                     .try_into()
//                     .unwrap()]);
//                     console.emit(superconsole::Lines(vec![line]));
//                     num_completed = num_completed + 1;
//                 }
//             }
//             let elapsed = start.elapsed().unwrap_or_else(|_| Duration::new(0, 0));
//             console
//                 .render(&InProgress::Logs(&elapsed, &log_tracker))
//                 .unwrap();
//             if num_completed == num_servers {
//                 break;
//             }
//         }
//
//         // TODO TODO TODO: Write all logs to a file and on finalize write out the file name where
//         // logs were written
//
//         // TODO TODO: Movethe finalize back to Setup::Done once done with testing
//
//         let elapsed = start.elapsed().unwrap_or_else(|_| Duration::new(0, 0));
//         console
//             .finalize(&InProgress::Logs(&elapsed, &log_tracker))
//             .unwrap();
//     }
// }
//
// fn reinstall(console: &mut SuperConsole, reinstall_path_str: &str) -> bool {
//     let pre_build_messages = vec![superconsole::Line::from_iter([
//         "Starting to re-install helloworld-server"
//             .to_string()
//             .with(Color::Magenta)
//             .try_into()
//             .unwrap(),
//     ])];
//     console.emit(superconsole::Lines(pre_build_messages));
//
//     console
//         .render(&Setup::Reinstall(reinstall_path_str))
//         .unwrap();
//
//     let stderr = std::process::Command::new(env!("CARGO"))
//         .arg("install")
//         .arg("--bin=helloworld-server")
//         .arg(format!("--path={}", reinstall_path_str))
//         .arg("--color=always")
//         .stderr(Stdio::piped())
//         .spawn()
//         .expect("Failed to start install command...")
//         .stderr
//         .expect("Failed to get a handle on install stderr..");
//
//     let mut reader = StdBufReader::new(stderr);
//
//     let reinstall_successful = 'outer: loop {
//         let mut lines = Vec::with_capacity(10);
//         for _ in 1..=10 {
//             let mut line = String::new();
//             match reader.read_line(&mut line) {
//                 Ok(0) => break 'outer true,
//                 Err(_) => {
//                     println!("Failed to read install output. Failling...");
//                     break 'outer false;
//                 }
//                 _ => (),
//             };
//
//             lines.push(line);
//         }
//
//         let lines = superconsole::Lines(
//             lines
//                 .into_iter()
//                 .map(|line| superconsole::Line::sanitized(&line))
//                 .collect(),
//         );
//         console.emit(lines);
//
//         console
//             .render(&Setup::Reinstall(reinstall_path_str))
//             .unwrap();
//     };
//
//     reinstall_successful
// }
//
// enum Message {
//     Log(LogMessage),
//     Close(CloseMessage),
// }
//
// struct LogMessage {
//     sender_id: usize,
//     message: String,
// }
//
// struct CloseMessage {
//     sender_id: usize,
// }
