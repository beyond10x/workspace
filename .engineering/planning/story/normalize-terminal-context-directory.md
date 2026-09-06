---
format: aep.planning-md/1
id: story:normalize-terminal-context-directory
kind: story
status: implemented
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
revision: 10
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

## Reviewed immutable publication

PR26 merged as 197e8ecb9d5492e21a2b0c3961b9f3cf26f09741 after policy CI 34059305825 passed. The annotated release tag 0.2.24 names exact reviewed and locally gated source e9de3e72581268d1ec5c93b2a95a68a435e58cc1. All six reviewed file hashes were checked before merge; the tagged source is an ancestor of the default branch and its tree matches the merge.

Release 34059345894 completed successfully, including AMD64 and ARM64 builds, native image smokes, keyless signing and exact workflow-certificate verification. The durable manifest binds workspace_service to sha256:5c5d2b4b161b740e798defa2f2058646398ef5796b688df77c4c134e8488620e at that reviewed source. Platform children are sha256:fe8f9dbae2312d9be5537a6eb5b8d836a0fb3545935bbca4a2af2fd26d4b2e99 for AMD64 and sha256:b09922e84e70e613c45a185b8e1686e82a7af7e015c10a2363cc927d10784232 for ARM64.

The downstream candidate changes only the Workspace version and digest in its existing lock, CI and values. Local validation using the exact published chart confirms that all eleven workload images agree with the lock. Independent private review, normal CI deployment and the actual post-terminal coding attempt remain required before implementation is claimed.

## Deployed context-boundary acceptance

The independently reviewed Workspace-only composition passed all three normal downstream jobs: immutable artifact and lock validation, atomic rollout, and deployed verification. The immediate before/after inventory retains thirteen workload identities and all durable claim identities; only the intended Workspace image changes to the published 0.2.24 digest. Other serving and initializer images, replicas, execution profiles and storage remain unchanged, and every workload is Ready.

Final authenticated headless acceptance creates a fresh Git workspace, reaches Ready in 23853 ms and the editor in 36683 ms. Project Files entry and persisted Agent-to-Files navigation pass. The 44-line editor layout, actual keyboard save with HTTP200, exact content/hash restoration and reload pass without JavaScript or content-security-policy errors.

The browser terminal receives initial PTY output after 502 ms, establishes canvas input focus, sends fifty binary input bytes and receives eleven binary output frames containing the complete split random printf sentinel. Explicit termination succeeds, and subsequent Workspace and file-tree reads remain Ready and HTTP200. The following coding-Agent request is admitted with HTTP202 and now reaches model_credential_unavailable after 3138 ms, instead of the predecessor's workspace_actor_view_unavailable. This observation and the deciding real ContextPack regression establish that the terminal-context boundary is repaired. The project-Agent request is admitted with HTTP200 and still fails.

The terminal is terminated, and both disposable Workspace and coordination are confirmed closed after one ordinary reconciliation retry. The initial cleanup-unknown result is retained. The preserved operator workspace remains Ready and readable before and after the run and is never edited or closed.

This is bounded acceptance of the terminal-directory correction. The final browser result remains DEPLOYMENT_ACCEPTANCE_FAILED because neither Agent produced its expected reply. The normal Claude credential reconnect and fresh successful replies remain an unresolved owner-dependent requirement of the overall product objective. No credential record is rewritten or blocker silently cleared. The retained final result has SHA-256 970d7d8fe951a1f13204e67aef25766d4d063e146607df133ed943285b62aa8c.
