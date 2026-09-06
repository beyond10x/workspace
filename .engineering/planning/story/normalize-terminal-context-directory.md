---
format: aep.planning-md/1
id: story:normalize-terminal-context-directory
kind: story
status: active
title: Normalize terminal directories at the AgentIDE context boundary
relations:
- informed_by: story:current-agentide-contract
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates/workspace-service/src/main.rs
- confidence: cited
  path: crates/workspace-service/src/terminal.rs
- confidence: cited
  path: crates/workspace-service/src/terminal_context_tests.rs
revision: 7
---
## Outcome

Preserve a valid coding-Agent context when an owned session has a running or terminated terminal. Keep the shell launch directory and terminal execution authority unchanged.

## Evidence

The first complete deployed journey after terminal admission recovery creates a terminal and displays a Bash prompt. A subsequent coding task is admitted and fails with workspace_actor_view_unavailable. In the released Workspace source, terminal profile validation requires the execution directory /workspace, and agentide_terminals copies that absolute directory into AgentIDE TerminalSession. AgentIDE's existing terminal validator requires a normalized workspace-relative path. ContextPack::seal validates every terminal, including terminated records, so this projection cannot produce a valid actor view. The predecessor AgentIDE contract has the same requirement; no new contract semantics or entity is introduced.

## Acceptance

A deciding regression traverses the real Workspace terminal projection and ContextPack sealing with an admitted terminal at the execution root. It fails with the predecessor's absolute projection and passes with the exact relative-root representation admitted by the current AgentIDE contract. Cover running and terminated records, preserving terminal identity, state and all other fields. Preserve launch validation and refusal for unsupported execution directories. Run the repository gate and independent review, publish an immutable Workspace release, review and deploy only its downstream pin, then verify real browser terminal command output and a subsequent coding attempt no longer fails at actor-view construction. Model credential redemption and successful replies remain separately required by overall Devcenter acceptance.

## Scope

crates/workspace-service/src/main.rs owns agentide_terminals and ContextPack construction. crates/workspace-service/src/terminal.rs owns the existing absolute launch-directory validation, which must remain intact. Cargo.toml, Cargo.lock and CHANGELOG.md own runtime release metadata. A focused projection/context regression may live in a dedicated terminal_context_tests.rs module. The coordinator owns planning mutations; the existing implementor owns runtime and test changes, followed by the existing independent reviewer.

## Implemented correction and local validation

The exact admitted absolute launch root /workspace now projects to the empty relative root required by AgentIDE. Valid running, exited and terminated rows retain their identity, profile, actor, process, state, output sequence and exit code. Unsupported stored directories explicitly refuse the complete projection with coding_terminal_directory_invalid. Both actor-view construction and terminal_list propagate the refusal. Existing launch and admission checks remain unchanged.

The deciding regression calls the production projection, ContextPack::seal and ContextPack::validate with all three terminal states. It reproduces terminal.session_invalid with the previous mapping and passes after the correction, checking every projected field and the unchanged absolute source directory. A valid preceding row plus an invalid stored directory proves that no partial projection escapes. A separate regression preserves rejection of every unsupported launch-directory form.

Full task check passes with 70 Workspace tests and 20 publication-policy/adversary tests, toolchain consistency, formatting and all-target Clippy. Six runtime/test/release-metadata files prepare version 0.2.24; only the three local Workspace package versions change in the lock. Immutable publication, reviewed downstream composition and real coding-context acceptance after terminal use remain required before this story is implemented.
