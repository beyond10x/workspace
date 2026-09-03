//! Deployment-declared terminal profiles and bounded in-memory replay.
//!
//! Durable lifecycle metadata lives in the Workspace store. Raw PTY output is deliberately
//! confined to this bounded process-local ring and disappears on service restart.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{Mutex, broadcast, mpsc};
use workspace_core::{TerminalExit, TerminalNetworkPosture, TerminalProfile};

pub const OUTPUT_RING_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROFILE_FILE_BYTES: u64 = 256 * 1024;
const MAX_PROFILES: usize = 32;
const ALLOWED_ENVIRONMENT: &[&str] = &["COLORTERM", "LANG", "LC_ALL", "TERM"];

/// Validated, immutable terminal profiles loaded before the listener starts.
#[derive(Clone, Default)]
pub struct TerminalProfiles(Arc<BTreeMap<String, TerminalProfile>>);

impl TerminalProfiles {
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let metadata = std::fs::metadata(path)
            .map_err(|_| "terminal profile file is unavailable".to_owned())?;
        if !metadata.is_file() || metadata.len() > MAX_PROFILE_FILE_BYTES {
            return Err("terminal profile file is not a bounded regular file".to_owned());
        }
        let bytes = std::fs::read(path)
            .map_err(|_| "terminal profile file could not be read".to_owned())?;
        let profiles = serde_json::from_slice::<Vec<TerminalProfile>>(&bytes)
            .map_err(|_| "terminal profile file is invalid".to_owned())?;
        if profiles.is_empty() || profiles.len() > MAX_PROFILES {
            return Err(format!(
                "terminal profile count must be within 1..={MAX_PROFILES}"
            ));
        }
        let mut by_id = BTreeMap::new();
        for profile in profiles {
            validate_profile(&profile)?;
            let id = profile.id.clone();
            if by_id.insert(id.clone(), profile).is_some() {
                return Err(format!("terminal profile `{id}` appears more than once"));
            }
        }
        Ok(Self(Arc::new(by_id)))
    }

    pub fn get(&self, id: &str) -> Option<&TerminalProfile> {
        self.0.get(id)
    }

    pub fn list(&self) -> Vec<TerminalProfile> {
        self.0.values().cloned().collect()
    }
}

fn validate_profile(profile: &TerminalProfile) -> Result<(), String> {
    if !valid_identifier(&profile.id)
        || profile.label.trim().is_empty()
        || profile.label.len() > 128
        || profile.runtime_ref.trim().is_empty()
        || profile.runtime_ref.len() > 256
        || !profile.shell.starts_with('/')
        || profile.shell.len() > 512
        || profile.working_directory != "/workspace"
        || profile.arguments.len() > 16
        || profile
            .arguments
            .iter()
            .any(|argument| argument.len() > 1_024 || argument.contains('\0'))
        || profile.network != TerminalNetworkPosture::None
    {
        return Err(format!("terminal profile `{}` is invalid", profile.id));
    }
    if !profile.environment.contains_key("TERM")
        || profile.environment.len() > ALLOWED_ENVIRONMENT.len()
        || profile.environment.iter().any(|(name, value)| {
            !ALLOWED_ENVIRONMENT.contains(&name.as_str())
                || value.is_empty()
                || value.len() > 256
                || value.contains('\0')
        })
    {
        return Err(format!(
            "terminal profile `{}` has an unsafe environment",
            profile.id
        ));
    }
    let limits = &profile.limits;
    let duration_bound =
        u64::try_from(b10x_substrate_sdk::MAX_EXEC_DURATION.as_millis()).unwrap_or(u64::MAX);
    if limits.timeout_ms == 0
        || limits.timeout_ms > duration_bound
        || limits.cpu_millis == 0
        || limits.cpu_millis > duration_bound
        || limits.memory_bytes < b10x_substrate_sdk::MIN_EXEC_MEMORY_BYTES
        || limits.memory_bytes > b10x_substrate_sdk::MAX_EXEC_MEMORY_BYTES
        || limits.processes == 0
        || limits.processes > b10x_substrate_sdk::MAX_EXEC_PROCESSES
        || limits.output_bytes == 0
        || limits.output_bytes > b10x_substrate_sdk::MAX_IO_BYTES
        || limits.input_bytes == 0
        || limits.input_bytes > b10x_substrate_sdk::MAX_SESSION_INPUT_BYTES
        || limits.frame_bytes == 0
        || limits.frame_bytes > b10x_substrate_sdk::MAX_SESSION_FRAME_BYTES
        || limits.queued_frames == 0
        || limits.queued_frames > b10x_substrate_sdk::MAX_SESSION_QUEUED_FRAMES
        || limits.lease_ttl_ms == 0
        || limits.lease_ttl_ms > duration_bound
    {
        return Err(format!(
            "terminal profile `{}` has limits outside the Substrate contract",
            profile.id
        ));
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

#[derive(Clone, Default)]
pub struct TerminalReplayHub {
    rings: Arc<Mutex<BTreeMap<String, ReplayRing>>>,
}

/// Browser-independent handle to the one live Substrate attachment owned by Workspace.
///
/// Dropping a browser's clone only detaches that browser. The broker task retains the upstream
/// channel until the process exits, explicit termination occurs, or the Workspace process stops.
#[derive(Clone)]
pub struct TerminalBroker {
    commands: mpsc::Sender<TerminalBrokerCommand>,
    events: broadcast::Sender<TerminalBrokerEvent>,
}

impl TerminalBroker {
    pub fn pair() -> (Self, mpsc::Receiver<TerminalBrokerCommand>) {
        let (commands, receiver) = mpsc::channel(128);
        let (events, _) = broadcast::channel(128);
        (Self { commands, events }, receiver)
    }

    pub async fn command(
        &self,
        command: TerminalBrokerCommand,
    ) -> Result<(), TerminalBrokerClosed> {
        self.commands
            .send(command)
            .await
            .map_err(|_| TerminalBrokerClosed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TerminalBrokerEvent> {
        self.events.subscribe()
    }

    pub fn publish(&self, event: TerminalBrokerEvent) {
        let _ = self.events.send(event);
    }
}

#[derive(Debug, thiserror::Error)]
#[error("terminal broker is no longer available")]
pub struct TerminalBrokerClosed;

#[derive(Clone)]
pub enum TerminalBrokerCommand {
    Input(Bytes),
    Resize { columns: u64, rows: u64 },
    Signal { signal: String, grace_ms: u64 },
}

#[derive(Clone)]
pub enum TerminalBrokerEvent {
    Output(SequencedOutput),
    Exit {
        observed_state: String,
        exit: Option<TerminalExit>,
    },
    Refused {
        code: String,
        substrate_code: Option<String>,
    },
    Detached {
        code: String,
    },
}

/// Registry of live broker handles. No authority credential or raw output is retained here.
#[derive(Clone, Default)]
pub struct TerminalBrokers(Arc<Mutex<BTreeMap<String, TerminalBroker>>>);

impl TerminalBrokers {
    pub async fn get(&self, terminal_id: &str) -> Option<TerminalBroker> {
        self.0.lock().await.get(terminal_id).cloned()
    }

    /// Insert a candidate, returning the already-live broker when a concurrent attach won.
    pub async fn insert(
        &self,
        terminal_id: &str,
        candidate: TerminalBroker,
    ) -> Result<(), TerminalBroker> {
        let mut brokers = self.0.lock().await;
        if let Some(existing) = brokers.get(terminal_id) {
            return Err(existing.clone());
        }
        brokers.insert(terminal_id.to_owned(), candidate);
        Ok(())
    }

    pub async fn remove(&self, terminal_id: &str) {
        self.0.lock().await.remove(terminal_id);
    }
}

impl TerminalReplayHub {
    pub async fn push(&self, terminal_id: &str, bytes: &[u8]) -> SequencedOutput {
        let mut rings = self.rings.lock().await;
        rings.entry(terminal_id.to_owned()).or_default().push(bytes)
    }

    pub async fn replay(&self, terminal_id: &str, after: Option<u64>) -> ReplaySnapshot {
        let rings = self.rings.lock().await;
        rings
            .get(terminal_id)
            .map_or_else(ReplaySnapshot::empty, |ring| ring.snapshot(after))
    }

    pub async fn remove(&self, terminal_id: &str) {
        self.rings.lock().await.remove(terminal_id);
    }
}

#[derive(Clone)]
pub struct SequencedOutput {
    pub sequence: u64,
    pub bytes: Vec<u8>,
}

pub struct ReplaySnapshot {
    pub earliest_sequence: Option<u64>,
    pub latest_sequence: Option<u64>,
    pub complete: bool,
    pub frames: Vec<SequencedOutput>,
}

impl ReplaySnapshot {
    fn empty() -> Self {
        Self {
            earliest_sequence: None,
            latest_sequence: None,
            complete: true,
            frames: Vec::new(),
        }
    }
}

#[derive(Default)]
struct ReplayRing {
    frames: VecDeque<SequencedOutput>,
    bytes: usize,
    next_sequence: u64,
}

impl ReplayRing {
    fn push(&mut self, bytes: &[u8]) -> SequencedOutput {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let frame = SequencedOutput {
            sequence: self.next_sequence,
            bytes: bytes.to_vec(),
        };
        self.bytes = self.bytes.saturating_add(frame.bytes.len());
        self.frames.push_back(frame.clone());
        while self.bytes > OUTPUT_RING_BYTES {
            let Some(removed) = self.frames.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.bytes.len());
        }
        frame
    }

    fn snapshot(&self, after: Option<u64>) -> ReplaySnapshot {
        let earliest_sequence = self.frames.front().map(|frame| frame.sequence);
        let latest_sequence = self.frames.back().map(|frame| frame.sequence);
        let complete = after.is_none_or(|sequence| {
            earliest_sequence.is_none_or(|earliest| sequence.saturating_add(1) >= earliest)
        });
        let frames = self
            .frames
            .iter()
            .filter(|frame| after.is_none_or(|sequence| frame.sequence > sequence))
            .cloned()
            .collect();
        ReplaySnapshot {
            earliest_sequence,
            latest_sequence,
            complete,
            frames,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> TerminalProfile {
        serde_json::from_value(serde_json::json!({
            "id": "rust-stable",
            "label": "Rust stable",
            "runtime_ref": "substrate:workspace-default@sha256:example",
            "shell": "/bin/sh",
            "arguments": ["-l"],
            "working_directory": "/workspace",
            "environment": {"TERM": "xterm-256color", "COLORTERM": "truecolor"},
            "workspace_access": "read_write",
            "network": "none",
            "limits": {
                "timeout_ms": 3_600_000,
                "cpu_millis": 3_600_000,
                "memory_bytes": 536_870_912,
                "processes": 256,
                "output_bytes": 1_048_576,
                "input_bytes": 16_777_216,
                "frame_bytes": 65_536,
                "queued_frames": 16,
                "lease_ttl_ms": 3_600_000
            }
        }))
        .expect("profile")
    }

    #[test]
    fn profile_requires_a_fixed_safe_shell_environment_and_substrate_bounds() {
        let admitted = profile();
        validate_profile(&admitted).expect("declared profile is admitted");

        let mut relative_shell = admitted.clone();
        relative_shell.shell = "sh".to_owned();
        assert!(validate_profile(&relative_shell).is_err());

        let mut ambient_environment = admitted.clone();
        ambient_environment
            .environment
            .insert("AWS_SECRET_ACCESS_KEY".to_owned(), "secret".to_owned());
        assert!(validate_profile(&ambient_environment).is_err());

        let mut oversized_queue = admitted;
        oversized_queue.limits.queued_frames =
            b10x_substrate_sdk::MAX_SESSION_QUEUED_FRAMES.saturating_add(1);
        assert!(validate_profile(&oversized_queue).is_err());
    }

    #[tokio::test]
    async fn replay_is_bounded_and_reports_an_evicted_cursor() {
        let hub = TerminalReplayHub::default();
        let oversized = vec![b'x'; OUTPUT_RING_BYTES];
        let first = hub.push("terminal-one", b"first").await;
        let last = hub.push("terminal-one", &oversized).await;
        let replay = hub
            .replay("terminal-one", Some(first.sequence.saturating_sub(1)))
            .await;
        assert!(!replay.complete);
        assert_eq!(replay.earliest_sequence, Some(last.sequence));
        assert_eq!(replay.frames.len(), 1);
    }

    #[tokio::test]
    async fn browser_clones_do_not_consume_the_single_broker_attachment() {
        let brokers = TerminalBrokers::default();
        let (broker, mut commands) = TerminalBroker::pair();
        assert!(
            brokers.insert("terminal-one", broker.clone()).await.is_ok(),
            "first broker wins"
        );
        let browser = brokers.get("terminal-one").await.expect("live broker");
        drop(browser);

        broker
            .command(TerminalBrokerCommand::Resize {
                columns: 120,
                rows: 40,
            })
            .await
            .expect("broker remains live after browser detach");
        assert!(matches!(
            commands.recv().await,
            Some(TerminalBrokerCommand::Resize {
                columns: 120,
                rows: 40
            })
        ));
    }
}
