---
format: aep.planning-md/1
id: review-result:independent-publication-pass-1
kind: review-result
status: active
title: Independent runtime publication adversarial pass 1
relations:
- reviews: story:independent-runtime-publication
revision: 1
---
unit: independent-runtime-publication — working tree based on f57e595
verdict: NEEDS-CHANGE
cases: executed 5→10, red 3
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 25 files under assigned scratch, listed below
needs-coordinator: wire new standalone adversary test into gate; route retry fixes to implementor

1. git --no-pager diff --stat
```
 README.md    | 17 +++++++++++++++++
 Taskfile.yml |  4 ++++
 2 files changed, 21 insertions(+)
```
Those are inherited frozen implementation changes, present before this pass. The only adversary change is the untracked test file, measured with `git diff --no-index --stat /dev/null ci/release_adversary.rs`:
```
 /dev/null => ci/release_adversary.rs | 146 +++++++++++++++++++++++++++++++++++
 1 file changed, 146 insertions(+)
```
No inherited implementation, assertion, configuration, planning, or Git state was changed. The new tests use an actual compiled release helper, real jq, and isolated subprocess stubs for provider/git reads. No remote API call or publication was performed.

2. Added executable cases and individual first runs

Compiled using `rustc --edition=2024 ci/release.rs -o "$TMPDIR/adversary-helper"` and `rustc --edition=2024 --test ci/release_adversary.rs -o "$TMPDIR/adversary-tests"`, with TMPDIR=/home/timo/.cache/independent-publication-wave.e1Bll4/workspace and ADV_HELPER=$TMPDIR/adversary-helper.

Each case was run with `$TMPDIR/adversary-tests --exact <name>` before the combined suite. First red output (before rustfmt shifted test source line numbers):
```
running 1 test
test successful_release_with_deleted_registry_package_is_not_a_completed_retry ... FAILED

failures:

---- successful_release_with_deleted_registry_package_is_not_a_completed_retry stdout ----

thread 'successful_release_with_deleted_registry_package_is_not_a_completed_retry' (1105856) panicked at ci/release_adversary.rs:50:5:
deleted package was accepted as completed: published=true
version=0.2.17
source=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
default_branch=trunk

note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    successful_release_with_deleted_registry_package_is_not_a_completed_retry

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.01s
```
Exit 101; remains red. Asserts a successful version cannot be considered available after its registry package disappears.

```
running 1 test
test draft_with_uploaded_immutable_manifest_does_not_restart_image_builds ... FAILED

failures:

---- draft_with_uploaded_immutable_manifest_does_not_restart_image_builds stdout ----

thread 'draft_with_uploaded_immutable_manifest_does_not_restart_image_builds' (1105868) panicked at ci/release_adversary.rs:57:5:
draft already has an uploaded immutable manifest but prepare unconditionally restarts both native builds
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    draft_with_uploaded_immutable_manifest_does_not_restart_image_builds

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.01s
```
Exit 101; remains red. Asserts recovery inspects an existing draft manifest before allocating image rebuilds.

```
running 1 test
test retry_after_manifest_upload_and_edit_failure_can_finish ... FAILED

failures:

---- retry_after_manifest_upload_and_edit_failure_can_finish stdout ----

thread 'retry_after_manifest_upload_and_edit_failure_can_finish' (1108792) panicked at ci/release_adversary.rs:94:5:
retry cannot finish uploaded draft: release refused: draft has conflicting immutable manifest; refusing overwrite

note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    retry_after_manifest_upload_and_edit_failure_can_finish

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.01s
```
Exit 101; remains red. Asserts recovery after upload success / final edit failure can finish using preserved receipts, even when the unnecessary fresh builds produce different digests. Fixture constructs differing receipts; no actual registry/build nondeterminism was measured.

Controls `forbidden_registry_is_not_bootstrap` and `matching_completed_release_skips_builds_with_provider_default_branch` each ran individually: one passed, zero failed, four filtered out, exit 0.

3. Scoped suite, after all cases existed

Command: `$TMPDIR/adversary-tests`, exit 101:
```
running 5 tests
test forbidden_registry_is_not_bootstrap ... ok
test draft_with_uploaded_immutable_manifest_does_not_restart_image_builds ... FAILED
test matching_completed_release_skips_builds_with_provider_default_branch ... ok
test retry_after_manifest_upload_and_edit_failure_can_finish ... FAILED
test successful_release_with_deleted_registry_package_is_not_a_completed_retry ... FAILED

failures:

---- draft_with_uploaded_immutable_manifest_does_not_restart_image_builds stdout ----

thread 'draft_with_uploaded_immutable_manifest_does_not_restart_image_builds' (1110289) panicked at ci/release_adversary.rs:84:5:
draft already has an uploaded immutable manifest but prepare unconditionally restarts both native builds
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- retry_after_manifest_upload_and_edit_failure_can_finish stdout ----

thread 'retry_after_manifest_upload_and_edit_failure_can_finish' (1110292) panicked at ci/release_adversary.rs:141:5:
retry cannot finish uploaded draft: release refused: draft has conflicting immutable manifest; refusing overwrite

---- successful_release_with_deleted_registry_package_is_not_a_completed_retry stdout ----

thread 'successful_release_with_deleted_registry_package_is_not_a_completed_retry' (1110293) panicked at ci/release_adversary.rs:69:5:
deleted package was accepted as completed: published=true
version=0.2.17
source=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
default_branch=trunk

failures:
    draft_with_uploaded_immutable_manifest_does_not_restart_image_builds
    retry_after_manifest_upload_and_edit_failure_can_finish
    successful_release_with_deleted_registry_package_is_not_a_completed_retry

test result: FAILED. 2 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```
Original scoped suite compiled with `rustc --edition=2024 --test ci/release.rs -o "$TMPDIR/adversary-policy-tests"`, then `$TMPDIR/adversary-policy-tests`, exit 0:
```
running 5 tests
test tests::exact_version_excludes_injection_and_floating_refs ... ok
test tests::immutable_digest_and_source_are_required ... ok
test tests::readiness_requires_successful_http_response ... ok
test tests::retry_requires_matching_source_and_complete_immutable_metadata ... ok
test tests::owner_workflow_smokes_before_pushing_and_limits_credentials ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
Count 5→10 refers to these scoped release lanes; original 42 runtime cases were not repeated. Before count comes from implementor report and was also measured after new test creation with adversary files excluded. `git diff --check` passed.

4. Findings against working tree based on f57e595

| file:line | verdict | origin | what was measured | what reaches it |
| --- | --- | --- | --- | --- |
| ci/release.rs:242 | NEEDS-CHANGE | introduced | Existing draft always emits published=false; actual announce with fresh differing receipts refuses at :393; new cases :84 and :141 exit 101. Interrupted draft recovery rebuilds images before inspecting existing immutable metadata, so fresh digests can make the draft impossible to complete. | announce uploads manifest at :399 before final edit at :402; a transient failure of that edit leaves exactly this draft; workflow images condition consumes published=false. Build reproducibility is not established or enforced. |
| ci/release.rs:240 | NEEDS-CHANGE | introduced | Missing package 404 plus authenticated empty listing still yields published=true when release metadata matches; case :69 exit 101. A completed release whose registry package is absent is accepted as a successful no-op, leaving its recorded digest unavailable. | Recovery dispatch after package deletion/retention, or package target misconfiguration to an absent package under the same owner; prepare allows bootstrap before its completed-release check. |

Both code paths were introduced: `git cat-file -e f57e595:ci/release.rs` reports that path does not exist at base. Neither finding recommends overwriting a manifest. Reuse and verify the stored draft receipts before rebuilding, and refuse completed identities when their registry artifact is unavailable.

5. Attacked and not broken

403 package API failure fails closed without bootstrap. Matching completed release skips image allocation and honors provider default branch `trunk`. Ordinary old-source recovery dispatch from default branch uses trusted helper in image and publication jobs; the initial missing-helper theory was discarded after checking workflow dispatch checkout semantics. Existing smoke checks both HTTP health and readiness; no additional runtime smoke was run in this pass. No claim was established about live native architecture receipt identity.

6. Outside-worktree writes (all under assigned scratch)

Root: /home/timo/.cache/independent-publication-wave.e1Bll4/workspace

- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-helper
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-tests
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-policy-tests
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass-1.md
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/release-manifest.json

```findings
- file: ci/release.rs
  line: 242
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Interrupted draft recovery rebuilds images before inspecting existing immutable metadata, so fresh digests can make the draft impossible to complete.
- file: ci/release.rs
  line: 240
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: A completed release whose registry package is absent is accepted as a successful no-op, leaving its recorded digest unavailable.
```
