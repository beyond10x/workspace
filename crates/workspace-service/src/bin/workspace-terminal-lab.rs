#![forbid(unsafe_code)]

//! Loopback-only browser terminal lab using the production Workspace replay/broker primitives and
//! a real, externally built Substrate daemon. It deliberately excludes Identity, `AgentIDE` grants,
//! and repository materialization so those authorities cannot be mistaken for tested behavior.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use b10x_substrate_sdk::{
    BaselineEnvironment, ExecutionPolicy, ManagedDaemon, PipeFrame, PtyWindow, Signal,
    WorkspaceAccess,
};
use bytes::Bytes;
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use workspace_core::TerminalExit;
use workspace_service::terminal::{
    TerminalBroker, TerminalBrokerCommand, TerminalBrokerEvent, TerminalReplayHub,
};

const TERMINAL_ID: &str = "terminal-review-1";
const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug, Parser)]
#[command(about = "Run a loopback Ghostty/Workspace/Substrate terminal lab")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8095")]
    listen: SocketAddr,
    #[arg(long)]
    substrate_daemon: PathBuf,
    #[arg(long)]
    cgroup_root: PathBuf,
}

#[derive(Clone)]
struct LabState {
    broker: TerminalBroker,
    replay: TerminalReplayHub,
    daemon_pid: Option<u32>,
    substrate_session_id: String,
    substrate_exec_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TerminalControl {
    Resize { columns: u64, rows: u64 },
    Signal { signal: String, grace_ms: u64 },
}

#[tokio::main]
#[allow(
    clippy::too_many_lines,
    reason = "the lab startup keeps its one explicit daemon/workspace/PTY ownership sequence visible"
)]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !args.listen.ip().is_loopback() {
        bail!("the terminal lab refuses a non-loopback listener");
    }
    if !args.substrate_daemon.is_file() {
        bail!("--substrate-daemon must name a built daemon binary");
    }
    if !args.cgroup_root.is_dir() {
        bail!("--cgroup-root must name the delegated cgroup owned by this lab");
    }

    let mut daemon = ManagedDaemon::builder()
        .temporary()
        .deployment("workspace_terminal_lab")
        .external_binary(&args.substrate_daemon)
        .cgroup_root(&args.cgroup_root)
        .start()
        .await
        .context("start the real Substrate daemon")?;
    if daemon.client().machine().facts.sessions_pty != Some(true) {
        bail!("Substrate did not publish its probe-verified sessions.pty capability");
    }

    let workspace = daemon
        .client()
        .workspace()
        .empty()
        .create()
        .await
        .context("create the confined lab workspace")?;
    workspace
        .write_file(
            "README.md",
            b"# Real Workspace terminal lab\n\nThese bytes live in Substrate, not Devcenter.\n",
        )
        .await
        .context("write the lab README through Substrate")?;
    workspace
        .write_file(
            "Cargo.toml",
            b"[package]\nname = \"workspace-terminal-lab\"\nversion = \"0.0.0\"\n",
        )
        .await
        .context("write the lab manifest through Substrate")?;

    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_hours(1))
        .cpu_time(Duration::from_hours(1))
        .memory_bytes(512 * 1024 * 1024)
        .processes(256)
        .output_bytes(1024 * 1024)
        .build()
        .context("construct the fixed terminal policy")?;
    let process = workspace
        .pty_session("/bin/sh", PtyWindow { columns: 100, rows: 30 })
        .args([
            "-c",
            "printf '\\033[2mReal Workspace → substrate-daemon PTY · network none\\033[0m\\r\\n'; exec /bin/sh -i",
        ])
        .allow_environment(BaselineEnvironment::Path)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .workspace_access(WorkspaceAccess::ReadWrite)
        .policy(policy)
        .lease(Duration::from_hours(1))
        .input_limit_bytes(16 * 1024 * 1024)
        .frame_limit_bytes(MAX_FRAME_BYTES as u64)
        .queued_frames(16)
        .start()
        .await
        .context("start the confined Substrate PTY")?;
    let substrate_session_id = process.id().to_owned();
    let substrate_exec_id = process.observation().exec_id.clone();
    let channel = process
        .attach()
        .await
        .context("attach the Workspace broker")?;
    let replay = TerminalReplayHub::default();
    let (broker, commands) = TerminalBroker::pair();
    tokio::spawn(run_broker(
        broker.clone(),
        replay.clone(),
        commands,
        channel,
    ));

    let state = LabState {
        broker,
        replay,
        daemon_pid: daemon.process_id(),
        substrate_session_id,
        substrate_exec_id,
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/api/project-terminals/{terminal_id}/attach", get(attach))
        .with_state(state);
    let listener = TcpListener::bind(args.listen)
        .await
        .context("bind the loopback terminal lab")?;
    println!(
        "workspace-terminal-lab listening on http://{} with real substrate-daemon pid {:?}",
        args.listen,
        daemon.process_id()
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serve the terminal lab")?;
    daemon
        .shutdown()
        .await
        .context("stop the Substrate daemon")?;
    Ok(())
}

async fn health(State(state): State<LabState>) -> impl IntoResponse {
    axum::Json(json!({
        "mode": "real_substrate_daemon",
        "terminal_id": TERMINAL_ID,
        "daemon_pid": state.daemon_pid,
        "substrate_session_id": state.substrate_session_id,
        "substrate_exec_id": state.substrate_exec_id,
        "network": "none",
    }))
}

async fn attach(
    State(state): State<LabState>,
    axum::extract::Path(terminal_id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::BTreeMap<String, String>>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    if terminal_id != TERMINAL_ID {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let from_sequence = match query.get("from_sequence") {
        Some(value) => match value.parse::<u64>() {
            Ok(value) => Some(value),
            Err(_) => return axum::http::StatusCode::UNPROCESSABLE_ENTITY.into_response(),
        },
        None => None,
    };
    upgrade
        .max_frame_size(MAX_FRAME_BYTES)
        .max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| terminal_socket(socket, state, from_sequence))
        .into_response()
}

async fn run_broker(
    broker: TerminalBroker,
    replay: TerminalReplayHub,
    mut commands: tokio::sync::mpsc::Receiver<TerminalBrokerCommand>,
    mut channel: b10x_substrate_sdk::PipeChannel,
) {
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let result = match command {
                    TerminalBrokerCommand::Input(bytes) => channel.write(bytes).await,
                    TerminalBrokerCommand::Resize { columns, rows } => {
                        channel.resize(PtyWindow { columns, rows }).await
                    }
                    TerminalBrokerCommand::Signal { signal, grace_ms } => {
                        let signal = match signal.as_str() {
                            "INT" => Signal::Interrupt,
                            "TERM" => Signal::Terminate,
                            "KILL" => Signal::Kill,
                            _ => break,
                        };
                        channel.signal(signal, Duration::from_millis(grace_ms)).await
                    }
                };
                if result.is_err() {
                    broker.publish(TerminalBrokerEvent::Detached {
                        code: "terminal_transport_unavailable".to_owned(),
                    });
                    break;
                }
            }
            frame = channel.next_frame() => {
                match frame {
                    Ok(Some(PipeFrame::Output { bytes, .. })) => {
                        let output = replay.push(TERMINAL_ID, &bytes).await;
                        broker.publish(TerminalBrokerEvent::Output(output));
                    }
                    Ok(Some(PipeFrame::Exit { state, exit, .. })) => {
                        broker.publish(TerminalBrokerEvent::Exit {
                            observed_state: format!("{state:?}").to_ascii_lowercase(),
                            exit: exit.map(|exit| TerminalExit {
                                code: exit.code.map(i32::from),
                                signal: exit.signal.map(|signal| format!("{signal:?}").to_ascii_uppercase()),
                            }),
                        });
                        break;
                    }
                    Ok(Some(PipeFrame::ProtocolError { code, .. })) => {
                        broker.publish(TerminalBrokerEvent::Refused {
                            code: "terminal_protocol_refused".to_owned(),
                            substrate_code: Some(code),
                        });
                        break;
                    }
                    Ok(Some(_)) => {
                        broker.publish(TerminalBrokerEvent::Refused {
                            code: "terminal_frame_unsupported".to_owned(),
                            substrate_code: None,
                        });
                        break;
                    }
                    Ok(None) | Err(_) => {
                        broker.publish(TerminalBrokerEvent::Detached {
                            code: "terminal_transport_unavailable".to_owned(),
                        });
                        break;
                    }
                }
            }
        }
    }
    let _ = channel.close().await;
}

async fn terminal_socket(mut socket: WebSocket, state: LabState, from_sequence: Option<u64>) {
    let mut events = state.broker.subscribe();
    let replay = state.replay.replay(TERMINAL_ID, from_sequence).await;
    if !send_json(
        &mut socket,
        json!({
            "kind": "attached",
            "mode": "real_substrate_daemon",
            "replay": {
                "complete": replay.complete,
                "oldest_sequence": replay.earliest_sequence,
                "newest_sequence": replay.latest_sequence,
            }
        }),
    )
    .await
    {
        return;
    }
    for frame in replay.frames {
        if !send_output(&mut socket, frame.sequence, &frame.bytes).await {
            return;
        }
    }
    loop {
        tokio::select! {
            browser = socket.recv() => {
                let Some(Ok(browser)) = browser else { break };
                let accepted = match browser {
                    Message::Binary(bytes) if !bytes.is_empty() && bytes.len() <= MAX_FRAME_BYTES => {
                        state.broker.command(TerminalBrokerCommand::Input(bytes)).await.is_ok()
                    }
                    Message::Text(text) => dispatch_control(&state.broker, &text).await,
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => true,
                    Message::Binary(_) => false,
                };
                if !accepted {
                    let _ = send_json(&mut socket, json!({
                        "kind": "refused",
                        "code": "terminal_input_refused",
                    })).await;
                    break;
                }
            }
            event = events.recv() => {
                match event {
                    Ok(TerminalBrokerEvent::Output(frame)) => {
                        if !send_output(&mut socket, frame.sequence, &frame.bytes).await { break; }
                    }
                    Ok(TerminalBrokerEvent::Exit { observed_state, exit }) => {
                        let _ = send_json(&mut socket, json!({
                            "kind": "exit",
                            "state": observed_state,
                            "exit": exit,
                        })).await;
                        break;
                    }
                    Ok(TerminalBrokerEvent::Refused { code, substrate_code }) => {
                        let _ = send_json(&mut socket, json!({
                            "kind": "refused",
                            "code": code,
                            "substrate_code": substrate_code,
                        })).await;
                        break;
                    }
                    Ok(TerminalBrokerEvent::Detached { code }) => {
                        let _ = send_json(&mut socket, json!({
                            "kind": "detached",
                            "code": code,
                        })).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = send_json(&mut socket, json!({
                            "kind": "refused",
                            "code": "terminal_slow_reader",
                        })).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn dispatch_control(broker: &TerminalBroker, text: &str) -> bool {
    match serde_json::from_str::<TerminalControl>(text) {
        Ok(TerminalControl::Resize { columns, rows })
            if PtyWindow { columns, rows }.within_bounds() =>
        {
            broker
                .command(TerminalBrokerCommand::Resize { columns, rows })
                .await
                .is_ok()
        }
        Ok(TerminalControl::Signal { signal, grace_ms })
            if matches!(signal.as_str(), "INT" | "TERM" | "KILL") && grace_ms <= 30_000 =>
        {
            broker
                .command(TerminalBrokerCommand::Signal { signal, grace_ms })
                .await
                .is_ok()
        }
        _ => false,
    }
}

async fn send_output(socket: &mut WebSocket, sequence: u64, bytes: &[u8]) -> bool {
    let mut frame = Vec::with_capacity(8 + bytes.len());
    frame.extend_from_slice(&sequence.to_be_bytes());
    frame.extend_from_slice(bytes);
    socket
        .send(Message::Binary(Bytes::from(frame)))
        .await
        .is_ok()
}

async fn send_json(socket: &mut WebSocket, value: serde_json::Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
