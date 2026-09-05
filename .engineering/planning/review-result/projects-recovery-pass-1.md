---
format: aep.planning-md/1
id: review-result:projects-recovery-pass-1
kind: review-result
status: active
title: Projects recovery adversarial pass
owner: aep-drive:adversary
relations:
- reviews: story:projects-connection-recovery
revision: 1
---
unit: Projects recovery — Devcenter 7dee588f78944163bf249d05d378fa15f94f4007; Workspace 197465aed4f70995bbf4d76b12081e12bd70f859 plus the test-only working-tree delta
verdict: nothing found
cases: executed 81→85, red 0
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: none
needs-coordinator: none

1. git --no-pager diff --stat

/home/timo/.local/state/worktree/trees/b10x/devcenter/projects-recovery-devcenter-20260905
```console
$ git --no-pager diff --stat
 frontend/e2e/devcenter.spec.ts | 28 ++++++++++++++++++++++++++++
 1 file changed, 28 insertions(+)
```

/home/timo/.local/state/worktree/trees/b10x/workspace/projects-recovery-workspace-20260905
```console
$ git --no-pager diff --stat
 .../src/repository_search_tests.rs                 | 79 ++++++++++++++++++++++
 1 file changed, 79 insertions(+)
```

This is my test-only delta against the handed-off implementation commits. The earlier implementation diffs against wave/projects-recovery were read separately; I did not edit those production changes, planning records, versions, or commits.

2. Cases added and their first selected runs

No suite ran before these cases existed. No failure was found. The isolated Workspace cases each selected one test and passed. The browser case selected both desktop and mobile and passed after correcting a command filter that initially selected no tests. The missed selection is retained below and is not a product finding.

All commands ran from the corresponding repository root. Workspace environment: RUSTC_WRAPPER=/usr/bin/sccache CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 TMPDIR=$PWD/.scratch/projects-recovery CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="env TMPDIR=../../.scratch/projects-recovery". Browser environment: PATH=/home/timo/.nvm/versions/node/v24.20.0/bin:$PATH TMPDIR=$PWD/.scratch/projects-recovery DEVCENTER_E2E_PORT=4373. Each repository uses its own target/scratch. No CARGO_TARGET_DIR was set.

crates/workspace-service/src/repository_search_tests.rs: a_newly_connected_operation_is_discovered_after_an_empty_recovery_result asserts that a prior recoverable empty result does not hide a newly connected operation on a later request; green.

```console
$ cargo test -p workspace-service --locked --bin workspace-service repository_search_tests::a_newly_connected_operation_is_discovered_after_an_empty_recovery_result -- --exact
   Compiling workspace-core v0.2.20 (/home/timo/.local/state/worktree/trees/b10x/workspace/projects-recovery-workspace-20260905/crates/workspace-core)
   Compiling workspace-service v0.2.20 (/home/timo/.local/state/worktree/trees/b10x/workspace/projects-recovery-workspace-20260905/crates/workspace-service)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.85s
     Running unittests src/main.rs (target/debug/deps/workspace_service-c852a7174313a4ce)

running 1 test
test repository_search_tests::a_newly_connected_operation_is_discovered_after_an_empty_recovery_result ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 52 filtered out; finished in 0.01s
exit: 0
```

crates/workspace-service/src/repository_search_tests.rs: branch_discovery::revoked_connection_discards_pages_and_is_not_reused_on_the_next_read asserts that NotGranted after a full page returns 403 without partial data, and a subsequent read re-describes authority and refuses the withdrawn connection without invocation; green.

```console
$ cargo test -p workspace-service --locked --bin workspace-service repository_search_tests::branch_discovery::revoked_connection_discards_pages_and_is_not_reused_on_the_next_read -- --exact
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/main.rs (target/debug/deps/workspace_service-c852a7174313a4ce)

running 1 test
test repository_search_tests::branch_discovery::revoked_connection_discards_pages_and_is_not_reused_on_the_next_read ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 52 filtered out; finished in 0.01s
exit: 0
```

Initial browser selection attempt: fully anchored grep matched no complete Playwright titles. No test ran; this runner-selection error is not a finding.

```console
$ pnpm --dir frontend test:e2e devcenter.spec.ts --workers=2 --grep '^keeps revoked access visible when opening a previously listed project$'
$ playwright test devcenter.spec.ts --workers=2 --grep '^keeps revoked access visible when opening a previously listed project$'
[WebServer] (node:2087441) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
[WebServer] (Use `node --trace-warnings ...` to show where the warning was created)
[WebServer] $ vite --host 127.0.0.1 --port 4373
[WebServer] (node:2087473) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
[WebServer] (Use `node --trace-warnings ...` to show where the warning was created)
Error: No tests found.
Make sure that arguments are regular expressions matching test files.
You may need to escape symbols like "$" or "*" and quote the arguments.

[ELIFECYCLE] Command failed with exit code 1.
exit: 1
```

frontend/e2e/devcenter.spec.ts: keeps revoked access visible when opening a previously listed project asserts that a 403 at existing-project open remains a visible authority refusal and does not navigate, offer empty recovery, or issue a create POST; green on desktop and mobile.

```console
$ pnpm --dir frontend test:e2e devcenter.spec.ts --workers=2 --grep 'keeps revoked access visible when opening a previously listed project'
$ playwright test devcenter.spec.ts --workers=2 --grep 'keeps revoked access visible when opening a previously listed project'
[WebServer] (node:2089883) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
[WebServer] (Use `node --trace-warnings ...` to show where the warning was created)
[WebServer] $ vite --host 127.0.0.1 --port 4373
[WebServer] (node:2089900) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
[WebServer] (Use `node --trace-warnings ...` to show where the warning was created)

Running 2 tests using 2 workers

(node:2089973) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
(Use `node --trace-warnings ...` to show where the warning was created)
(node:2089972) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
(Use `node --trace-warnings ...` to show where the warning was created)
  ✓  1 [mobile-chromium] › e2e/devcenter.spec.ts:1308:1 › keeps revoked access visible when opening a previously listed project (665ms)
  ✓  2 [chromium] › e2e/devcenter.spec.ts:1308:1 › keeps revoked access visible when opening a previously listed project (712ms)

  2 passed (2.2s)
exit: 0
```

3. Suite runs after the new cases

The before counts come from the implementor report and coordinator handoff, not a pre-addition run by this adversary: workspace-service 54 (library 3 + main 51), browser 27 executed with 15 existing skips. After the additions: workspace-service 56 (library 3 + main 53), browser 29 executed with the same 15 skips. The header sums only these two suites: 81→85. The unchanged frontend unit suite was not repeated. The new browser case runs on both projects, so the three added source cases produce four additional executed cases.

```console
$ cargo test -p workspace-service --locked
   Compiling workspace-service v0.2.20 (/home/timo/.local/state/worktree/trees/b10x/workspace/projects-recovery-workspace-20260905/crates/workspace-service)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.30s
     Running unittests src/lib.rs (target/debug/deps/workspace_service-89699b67ae238a70)

running 3 tests
test terminal::tests::profile_requires_a_fixed_safe_shell_environment_and_substrate_bounds ... ok
test terminal::tests::browser_clones_do_not_consume_the_single_broker_attachment ... ok
test terminal::tests::replay_is_bounded_and_reports_an_evicted_cursor ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running unittests src/main.rs (target/debug/deps/workspace_service-c852a7174313a4ce)

running 53 tests
test aep::tests::declared_aep_pins_name_the_released_contract_graph ... ok
test aep::tests::every_locked_aep_crate_resolves_from_that_one_release ... ok
test repository_search_tests::branch_discovery::an_exact_full_page_requires_a_final_empty_page ... ok
test store::tests::canonical_project_converges_but_requires_each_subject_association ... ok
test store::tests::a_late_materialization_reference_reopens_closed_cleanup_for_retry ... ok
test store::tests::closing_a_ready_session_retains_provenance_but_clears_live_references ... ok
test store::tests::coding_session_cannot_publish_without_materialization_and_refusal_clears_reference ... ok
test store::tests::coding_session_reservation_is_exact_owned_and_publishes_one_ref_atomically ... ok
test store::tests::materialization_cleanup_inventory_includes_and_deduplicates_legacy_references ... ok
test store::tests::message_task_stream_association_is_exact_and_owner_scoped ... ok
test store::tests::persisted_session_source_survives_a_later_project_snapshot ... ok
test repository_search_tests::branch_discovery::the_project_connection_must_be_currently_admitted ... ok
test repository_search_tests::absent_gitlab_operation_returns_empty_repositories_without_invoking ... ok
test repository_search_tests::unexpected_describe_result_remains_a_protocol_error ... ok
test repository_search_tests::malformed_provider_output_remains_a_protocol_error ... ok
test store::tests::schema_upgrade_accepts_legacy_workflow_task_rows ... ok
test tests::coding_sessions_admit_only_the_exact_provider_default_branch ... ok
test tests::file_operations_are_sealed_to_expected_state_and_content ... ok
test repository_search_tests::a_newly_connected_operation_is_discovered_after_an_empty_recovery_result ... ok
test tests::installs_the_process_crypto_provider_before_tls_clients_are_built ... ok
test tests::coding_session_and_agent_grant_are_derived_from_server_records ... ok
test tests::materialization_labels_are_bounded_before_substrate_operations ... ok
test tests::materialization_paths_refuse_git_metadata_and_escapes ... ok
test store::tests::predecessor_session_source_is_backfilled_only_from_the_same_exact_snapshot ... ok
test tests::materialization_workers_are_single_flight_and_recoverable ... ok
test repository_search_tests::connected_repository_search_keeps_current_authority_and_bounded_candidates ... ok
test tests::public_listener_admits_only_https_or_internal_cluster_identity ... ok
test tests::repository_candidate_keeps_provider_default_branch ... ok
test store::tests::refresh_records_a_commit_boundary_without_exposing_personal_threads ... ok
test tests::substrate_authority_uses_identity_canonical_runtime_scopes ... ok
test repository_search_tests::branch_discovery::revoked_connection_discards_pages_and_is_not_reused_on_the_next_read ... ok
test tests::source_manifest_commits_to_the_exact_revision ... ok
test tests::substrate_unified_diff_is_projected_with_exact_line_counts ... ok
test tests::workflow_task_terminal_states_project_to_named_safe_results ... ok
test tests::terminal_authority_refuses_actor_session_risk_scope_and_expiry_spoofing ... ok
test repository_search_tests::branch_discovery::pages_bind_exact_project_connection_and_fresh_descriptions ... ok
test store::tests::terminal_reservation_is_exact_idempotent_and_owner_scoped ... ok
test repository_search_tests::invocation_not_found_and_provider_failures_never_become_empty_success ... ok
test repository_search_tests::connector_http_failures_remain_errors ... ok
test store::tests::workflow_idempotency_covers_the_exact_snapshot_intent ... ok
test tests::git_materialization_recovers_one_exact_bounded_secret_free_create ... ok
test repository_search_tests::branch_discovery::missing_or_stale_branch_authority_remains_an_error ... ok
test store::tests::workflow_task_recovery_and_terminal_completion_are_durable_and_single_assignment ... ok
test repository_search_tests::branch_discovery::project_ids_are_positive_integers_before_connector_calls ... ok
test repository_search_tests::branch_discovery::later_page_failures_never_return_a_partial_list ... ok
test repository_search_tests::describe_authority_and_provider_refusals_remain_errors ... ok
test repository_search_tests::malformed_describe_not_found_envelopes_remain_errors ... ok
test tests::recoverable_workflow_survives_a_session_without_agent_platform_authority ... ok
test tests::long_running_workflow_remains_recoverable_when_observation_window_ends ... ok
test tests::ambiguous_workflow_submit_failure_remains_idempotently_retryable ... ok
test tests::persisted_workflow_task_resumes_with_fresh_observer_and_session ... ok
test repository_search_tests::branch_discovery::malformed_later_pages_never_return_a_partial_list ... ok
test tests::transient_workflow_observation_failure_remains_recoverable ... ok

test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.66s

     Running unittests src/bin/workspace-terminal-lab.rs (target/debug/deps/workspace_terminal_lab-c77a90de262c7679)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests workspace_service

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit: 0
```

```console
$ pnpm --dir frontend test:e2e devcenter.spec.ts --workers=2
$ playwright test devcenter.spec.ts --workers=2
[WebServer] (node:2095323) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
[WebServer] (Use `node --trace-warnings ...` to show where the warning was created)
[WebServer] $ vite --host 127.0.0.1 --port 4373
[WebServer] (node:2095343) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
[WebServer] (Use `node --trace-warnings ...` to show where the warning was created)

Running 44 tests using 2 workers

(node:2095472) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
(Use `node --trace-warnings ...` to show where the warning was created)
(node:2095473) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
(Use `node --trace-warnings ...` to show where the warning was created)
  ✓   1 [chromium] › e2e/devcenter.spec.ts:1067:1 › renders a signed-out authority path instead of an empty shell (1.1s)
  ✓   2 [chromium] › e2e/devcenter.spec.ts:1080:1 › opens a deep-linked agent and creates a governed worker (1.4s)
  ✓   4 [chromium] › e2e/devcenter.spec.ts:1133:1 › installs published starter graphs into an empty Workflow library (590ms)
  ✓   3 [chromium] › e2e/devcenter.spec.ts:1108:1 › inspects standalone Workflow definitions separately from project runs (1.5s)
  ✓   5 [chromium] › e2e/devcenter.spec.ts:1147:1 › runs the SDK-generated Todo console through the live BFF binding (2.2s)
  ✓   6 [chromium] › e2e/devcenter.spec.ts:1168:1 › reviews and approves only the exact suspended agent call (2.4s)
  ✓   7 [chromium] › e2e/devcenter.spec.ts:1235:1 › offers GitLab connection recovery for an empty repository listing (1.1s)
  ✓   8 [chromium] › e2e/devcenter.spec.ts:1256:1 › retains no-match wording for an empty repository search (524ms)
  ✓   9 [chromium] › e2e/devcenter.spec.ts:1269:1 › keeps Projects service failure distinct from empty connection recovery (543ms)
  ✓  10 [chromium] › e2e/devcenter.spec.ts:1284:1 › opens an already opened repository without creating another project (593ms)
  ✓  11 [chromium] › e2e/devcenter.spec.ts:1308:1 › keeps revoked access visible when opening a previously listed project (548ms)
  ✓  12 [chromium] › e2e/devcenter.spec.ts:1336:1 › opens a visible repository as a commit-pinned project (1.4s)
  ✓  13 [chromium] › e2e/devcenter.spec.ts:1366:1 › advances an accepted workflow and preserves its rendered report (3.1s)
  ✓  15 [chromium] › e2e/devcenter.spec.ts:1501:1 › refuses an actor-private coding session before mounting AgentIDE (490ms)
  ✓  14 [chromium] › e2e/devcenter.spec.ts:1386:1 › drives the AgentIDE v2 workbench over the Devcenter host port (6.3s)
  -  17 [chromium] › e2e/devcenter.spec.ts:1605:1 › keeps catalog and connection custody usable on a mobile viewport
  ✓  18 [chromium] › e2e/devcenter.spec.ts:1637:1 › makes capability posture explicit and applies bulk changes atomically (1.1s)
  ✓  16 [chromium] › e2e/devcenter.spec.ts:1521:1 › opens, resumes, and tears down the AgentIDE terminal byte channel (5.8s)
  ✓  19 [chromium] › e2e/devcenter.spec.ts:1673:1 › persists themes and makes search and navigation shortcuts discoverable (1.7s)
  ✓  20 [chromium] › e2e/devcenter.spec.ts:1747:1 › shows one stable MCP endpoint and least-privilege client setup (1.1s)
  ✓  21 [chromium] › e2e/devcenter.spec.ts:1779:1 › opens an editable file while coordination, layout, and terminals are still loading (2.7s)
(node:2097091) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
(Use `node --trace-warnings ...` to show where the warning was created)
  ✓  23 [mobile-chromium] › e2e/devcenter.spec.ts:1067:1 › renders a signed-out authority path instead of an empty shell (1.0s)
  -  24 [mobile-chromium] › e2e/devcenter.spec.ts:1080:1 › opens a deep-linked agent and creates a governed worker
  -  25 [mobile-chromium] › e2e/devcenter.spec.ts:1108:1 › inspects standalone Workflow definitions separately from project runs
  -  26 [mobile-chromium] › e2e/devcenter.spec.ts:1133:1 › installs published starter graphs into an empty Workflow library
  -  27 [mobile-chromium] › e2e/devcenter.spec.ts:1147:1 › runs the SDK-generated Todo console through the live BFF binding
  -  28 [mobile-chromium] › e2e/devcenter.spec.ts:1168:1 › reviews and approves only the exact suspended agent call
  ✓  22 [chromium] › e2e/devcenter.spec.ts:1823:1 › prepares a new coding session automatically and requests the tree only when ready (4.6s)
(node:2097318) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
(Use `node --trace-warnings ...` to show where the warning was created)
  ✓  29 [mobile-chromium] › e2e/devcenter.spec.ts:1235:1 › offers GitLab connection recovery for an empty repository listing (997ms)
  ✓  31 [mobile-chromium] › e2e/devcenter.spec.ts:1269:1 › keeps Projects service failure distinct from empty connection recovery (472ms)
  ✓  30 [mobile-chromium] › e2e/devcenter.spec.ts:1256:1 › retains no-match wording for an empty repository search (630ms)
  ✓  32 [mobile-chromium] › e2e/devcenter.spec.ts:1284:1 › opens an already opened repository without creating another project (555ms)
  -  34 [mobile-chromium] › e2e/devcenter.spec.ts:1336:1 › opens a visible repository as a commit-pinned project
  -  35 [mobile-chromium] › e2e/devcenter.spec.ts:1366:1 › advances an accepted workflow and preserves its rendered report
  -  36 [mobile-chromium] › e2e/devcenter.spec.ts:1386:1 › drives the AgentIDE v2 workbench over the Devcenter host port
  ✓  33 [mobile-chromium] › e2e/devcenter.spec.ts:1308:1 › keeps revoked access visible when opening a previously listed project (570ms)
  -  37 [mobile-chromium] › e2e/devcenter.spec.ts:1501:1 › refuses an actor-private coding session before mounting AgentIDE
  -  38 [mobile-chromium] › e2e/devcenter.spec.ts:1521:1 › opens, resumes, and tears down the AgentIDE terminal byte channel
  ✓  39 [mobile-chromium] › e2e/devcenter.spec.ts:1605:1 › keeps catalog and connection custody usable on a mobile viewport (1.2s)
  ✓  40 [mobile-chromium] › e2e/devcenter.spec.ts:1637:1 › makes capability posture explicit and applies bulk changes atomically (1.2s)
  -  41 [mobile-chromium] › e2e/devcenter.spec.ts:1673:1 › persists themes and makes search and navigation shortcuts discoverable
  -  42 [mobile-chromium] › e2e/devcenter.spec.ts:1747:1 › shows one stable MCP endpoint and least-privilege client setup
  -  43 [mobile-chromium] › e2e/devcenter.spec.ts:1779:1 › opens an editable file while coordination, layout, and terminals are still loading
  -  44 [mobile-chromium] › e2e/devcenter.spec.ts:1823:1 › prepares a new coding session automatically and requests the tree only when ready

  15 skipped
  29 passed (26.1s)
exit: 0
```

```console
$ cargo fmt --all --check

exit: 0
```

4. Findings

Nothing found in the bounded attacks. There are no judgement findings, confirmed defects, or origin-routing requests. These results cover the two implementation commits named in the header plus the test-only delta; they are test-runner outcomes, not approval.

5. What was attacked and could not be broken

- Recovery after connection state changes: two HTTP repository requests prove that Describe NotFound is recoverable without making the next connected result sticky-empty.
- Authority withdrawal during branch pagination: a real HostedClient error envelope after 100 records yields 403, and the next read performs a fresh Describe and refuses the no-longer-admitted project connection. Normal current-grant changes can reach this boundary through branches and select_branch after accessible_project revalidation.
- Access withdrawal after an existing project appears in the repository list: the browser keeps the 403 visible and stays on Projects with no create POST, on desktop and mobile. openRepository rechecks api.project before navigation, so this is a reachable between-request state.
- The current operation catalogue matches the numeric project/page/per_page inputs and raw array output. Existing tests continue to exercise exact project identity, fresh witnesses, complete pagination, malformed responses, and error propagation.
- Hosted elapsed-time and actual deployment verification remain outside these local fixtures and belong to the coordinator.

6. Paths written outside the assigned worktrees

None. Added source paths are only frontend/e2e/devcenter.spec.ts and crates/workspace-service/src/repository_search_tests.rs in their assigned trees. Report, command logs and exit records are under each assigned .scratch/projects-recovery; test/build output remains in the repository-owned target and frontend test-results locations. No planning writes, production edits, source mutations, commits, branch changes, new agents, or cleanup were performed. Tool cost/token counts were not exposed.

7. Findings block

```findings
[]
```
