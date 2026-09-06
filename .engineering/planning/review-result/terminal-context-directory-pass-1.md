---
format: aep.planning-md/1
id: review-result:terminal-context-directory-pass-1
kind: review-result
status: active
title: Independent terminal context directory source review
relations:
- reviews: story:normalize-terminal-context-directory
revision: 1
---
approve

No actionable findings in the final Workspace 0.2.24 terminal context-directory correction. The previously reported absolute-versus-relative path defect is resolved at the projection boundary.

Scope: the six runtime/test/release files against 050d8adf82a4565692002fcd7e2b7f3d9be62c71, excluding .engineering. The new crates/workspace-service/src/terminal_context_tests.rs was untracked and was inspected in full. Complete six-file full-index patch SHA-256: 4cef1bd244d9b9e79bd8e7f805f36f9e7f1af9b5ce3e04758c3ad0257d8c7fd8, computed by combining the tracked patch and the new file's full-index addition, sorted by file section. Tracked-only patch SHA-256: 10350dbd2d2b439cea1be7a35b84f9f83cee219a6f7fd5fc7b63a22b4d8562f9.

In crates/workspace-service/src/main.rs:1089, an otherwise projectable terminal must carry the exact admitted runtime root /workspace. Only that directory is translated into the canonical empty workspace-relative root. The implementation does not strip arbitrary prefixes, accept a dot as root, normalize unsupported paths, or omit an unsupported directory from an otherwise successful projection. A directory mismatch returns HTTP 502 coding_terminal_directory_invalid and discards the collected terminal result.

Running, Exited, and Terminated records remain represented. All other projected fields, including identity, session, profile, actor, process identity, state, network posture, output sequence, and exit code, retain their previous behavior. Existing foreign-session and nonprojectable-lifecycle handling remains unchanged. Both production callers propagate the new Result: coding_actor_view at main.rs:1298 and the terminal_list semantic intent at :1741 return the explicit refusal before exposing a partial result.

The exact absolute /workspace launch requirement in crates/workspace-service/src/terminal.rs is unchanged; its only edit is an additional regression test. No terminal launch argument, stored profile, admission/grant/binding predicate, runtime confinement, filesystem policy, dependency seam, or session state transition changes.

The deciding test in crates/workspace-service/src/terminal_context_tests.rs passes actual stored-terminal records through agentide_terminals into the real pinned ContextPack, then calls both seal and validate. It retains all three valid terminal states, including Terminated; compares every projected field; verifies there is no substituted activity-only record; and confirms each original profile remains at /workspace. The predecessor failed this test with terminal.session_invalid and exit 101. The corrected focused suite passes both projection tests.

The negative projection test places an admitted terminal before an unsupported terminated record, proving no partial result escapes. It covers empty, dot, relative, foreign absolute, child, trailing-slash, traversal, prefix-collision, and backslash spellings. The separate launch-admission test confirms the absolute root remains admitted and relative or unsupported launch directories remain refused.

The retained complete task check exits zero and includes toolchain and formatting checks, Clippy with warnings denied, 70 workspace tests, and 20 release-policy/adversary tests. Final file bytes independently match the implementor's tested source manifest. No builds or tests were rerun for this review.

Cargo.toml, Cargo.lock, and CHANGELOG.md prepare 0.2.24. Cargo.lock changes only the three local package versions; every other package identity and lock field is unchanged.

Limitations: these tests establish the production projection and real AgentIDE context validation without allocating a terminal or running an Agent. Publication, deployment, and a subsequent browser coding turn remain separate acceptance steps. This correction does not establish model-credential availability or successful Agent replies. The original diagnostic report remains immutable.

Reviewed file SHA-256 values:
- Cargo.toml: 09995c09744a7dce52eac5a6378d8eda37525686369455576eb8a2fe50a4cdef
- Cargo.lock: e45e101cbb3d4e202eb40d208571ad83fa0638a2d79136bde4ecbd816e2520fc
- CHANGELOG.md: 63c39e5ad7a5f897f016c18ebf4bc9612d459abe96984d7feda22870b094a518
- crates/workspace-service/src/main.rs: ff3482ed3064268b48af2b1679e133d453a17057e32719959e8d088018739669
- crates/workspace-service/src/terminal.rs: ae670119bd9310d8ec05ae668d77676dd752321b189a3ec73b5bdc9f7e06f675
- crates/workspace-service/src/terminal_context_tests.rs: 8e0583843ca953546f36b6b5b48932005dc6c7650dcd5a3b4ec7a84fee57c130

No repository, planning, Git, integration, credential, session, or deployment mutation was performed.

```findings
[]
```

