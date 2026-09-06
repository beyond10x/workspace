use super::*;

fn terminal(id: &str, state: TerminalState, exit_code: Option<i32>) -> StoredTerminal {
    let profile = serde_json::from_value(serde_json::json!({
        "id": "shell-profile", "label": "Confined shell",
        "runtime_ref": "substrate:workspace-default@sha256:example",
        "shell": "/bin/sh", "arguments": ["-l"],
        "working_directory": "/workspace",
        "environment": {"TERM": "xterm-256color"},
        "workspace_access": "read_write", "network": "none",
        "limits": {
            "timeout_ms": 60_000, "cpu_millis": 60_000,
            "memory_bytes": 134_217_728, "processes": 64,
            "output_bytes": 1_048_576, "input_bytes": 1_048_576,
            "frame_bytes": 65_536, "queued_frames": 16, "lease_ttl_ms": 60_000
        }
    }))
    .expect("terminal profile");
    StoredTerminal {
        public: TerminalSession {
            id: id.to_owned(),
            coding_session_id: "workspace-session-one".to_owned(),
            agentide_session_id: "agentide-session-one".to_owned(),
            authority_grant_id: "grant-one".to_owned(),
            profile,
            actor: "person:owner".to_owned(),
            process_id: Some(format!("process-{id}")),
            state,
            exit: exit_code.map(|code| TerminalExit {
                code: Some(code),
                signal: None,
            }),
            failure_code: None,
            created_at_ms: 1,
            updated_at_ms: 2,
        },
        substrate_session_ref: Some(format!("substrate-{id}")),
        initial_columns: 80,
        initial_rows: 24,
    }
}

fn context(terminals: Vec<StoredTerminal>) -> ContextPack {
    let mut recent_activity = Vec::new();
    let terminals = agentide_terminals(terminals, "agentide-session-one", &mut recent_activity)
        .expect("admitted terminal directories");
    ContextPack {
        format: "agentide.context-pack/2".to_owned(),
        objective: "Inspect the project".to_owned(),
        source_revision: "a".repeat(40),
        working_changes: None,
        pins: Vec::new(),
        focused_selections: Vec::new(),
        open_files: Vec::new(),
        active_diff: None,
        terminals,
        processes: Vec::new(),
        agent_lanes: Vec::new(),
        approvals: Vec::new(),
        evidence: Vec::new(),
        recent_activity,
        revision: 1,
        digest: String::new(),
    }
}

#[test]
fn unsupported_persisted_directories_refuse_the_complete_terminal_projection() {
    for path in [
        "",
        ".",
        "workspace",
        "/",
        "/other",
        "/workspace/",
        "/workspace/src",
        "/workspace/../other",
        "/workspace-other",
        "/workspace\\src",
    ] {
        let admitted = terminal("running", TerminalState::Running, None);
        let mut unsupported = terminal("terminated", TerminalState::Terminated, Some(137));
        unsupported.public.profile.working_directory = path.to_owned();
        let mut recent_activity = Vec::new();
        let response = agentide_terminals(
            vec![admitted, unsupported],
            "agentide-session-one",
            &mut recent_activity,
        )
        .expect_err("unsupported directories cannot become normalized or omitted terminals");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{path:?}");
        assert!(recent_activity.is_empty());
    }
}

#[test]
fn projected_running_and_finished_terminals_seal_context_at_workspace_root() {
    let records = vec![
        terminal("running", TerminalState::Running, None),
        terminal("exited", TerminalState::Exited, Some(0)),
        terminal("terminated", TerminalState::Terminated, Some(137)),
    ];
    let mut pack = context(records.clone());
    pack.seal()
        .expect("projected terminals must allow context sealing");
    pack.validate().expect("sealed terminal context is valid");
    assert_eq!(pack.terminals.len(), records.len());
    assert!(pack.recent_activity.is_empty());
    for (projected, (stored, state)) in pack.terminals.iter().zip(records.iter().zip([
        AgentIdeTerminalState::Running,
        AgentIdeTerminalState::Exited,
        AgentIdeTerminalState::Terminated,
    ])) {
        assert_eq!(stored.public.profile.working_directory, "/workspace");
        assert_eq!(
            projected,
            &AgentIdeTerminalSession {
                format: "agentide.terminal-session/2".to_owned(),
                id: stored.public.id.clone(),
                session_id: stored.public.agentide_session_id.clone(),
                profile: stored.public.profile.id.clone(),
                actor: ActorContext::new(ActorKind::Human, stored.public.actor.clone()).unwrap(),
                process_id: stored.public.process_id.clone().unwrap(),
                working_directory: String::new(),
                network: "none".to_owned(),
                state,
                output_sequence: 0,
                exit_code: stored.public.exit.as_ref().and_then(|exit| exit.code),
            }
        );
    }
}
