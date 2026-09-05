---
format: aep.planning-md/1
id: review-result:independent-publication-pass-2
kind: review-result
status: active
title: Independent runtime publication adversarial pass 2
relations:
- reviews: story:independent-runtime-publication
revision: 1
---
unit: independent-runtime-publication — second/final pass, corrected working tree based on f57e595
verdict: nothing found
cases: executed 13→17, red 0
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: 107 files under assigned scratch, listed below
needs-coordinator: none

1. git --no-pager diff --stat
```
 .engineering/planning/journal.jsonl |  3 +++
 README.md                           | 19 +++++++++++++++++++
 Taskfile.yml                        |  8 ++++++++
 3 files changed, 30 insertions(+)
```
All these tracked changes were inherited before this pass; no implementation or planning file was edited by the adversary. Only ci/release_adversary.rs changed in this pass, appending four tests and one fixture replay helper to the existing eight cases. No existing case/assertion was removed, changed, weakened or skipped. The test file is untracked in this shared tree and now has 311 lines; tracked diff alone does not expose it. No Git branch, commit, cleanup, cluster, or external service operation occurred.

2. Added cases, individually executed before the suite

Commands:
```
export TMPDIR=/home/timo/.cache/independent-publication-wave.e1Bll4/workspace
export ADV_HELPER=$TMPDIR/adversary-helper
rustc --edition=2024 ci/release.rs -o "$ADV_HELPER"
rustc --edition=2024 --test ci/release_adversary.rs -o "$TMPDIR/adversary-tests"
```

Each case below ran with `"$TMPDIR/adversary-tests" --exact CASE`, exit 0:

```
running 1 test
test malformed_uploaded_receipt_refuses_completed_and_draft_recovery ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.46s

running 1 test
test different_receipt_source_refuses_completed_and_draft_recovery ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.07s

running 1 test
test moved_tag_refuses_before_completed_receipt_can_skip_builds ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.03s

running 1 test
test source_outside_default_branch_refuses_before_receipt_reuse ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.02s
```

These use the actual compiled release helper and jq. Test-owned fixture scripts model provider and Git reads. Malformed JSON and source mismatch are each enumerated across completed and draft states. The other cases independently refuse moved tags and non-ancestor sources before receipt reuse. No new red output exists.

3. Scoped suite after cases existed

Command: `"$TMPDIR/adversary-tests"`, exit 0:
```
running 12 tests
test forbidden_registry_is_not_bootstrap ... ok
test successful_release_with_deleted_registry_package_is_not_a_completed_retry ... ok
test draft_with_uploaded_immutable_manifest_does_not_restart_image_builds ... ok
test matching_completed_release_skips_builds_with_provider_default_branch ... ok
test source_outside_default_branch_refuses_before_receipt_reuse ... ok
test moved_tag_refuses_before_completed_receipt_can_skip_builds ... ok
test completed_and_draft_receipts_refuse_unavailable_artifacts ... ok
test retry_after_manifest_upload_and_edit_failure_can_finish ... ok
test completed_and_draft_receipts_refuse_wrong_index_membership ... ok
test completed_and_draft_receipts_require_owner_signature ... ok
test different_receipt_source_refuses_completed_and_draft_recovery ... ok
test malformed_uploaded_receipt_refuses_completed_and_draft_recovery ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

Command: `rustc --edition=2024 --test ci/release.rs -o "$TMPDIR/adversary-policy-tests"` then `"$TMPDIR/adversary-policy-tests"`, exit 0:
```
running 5 tests
test tests::exact_version_excludes_injection_and_floating_refs ... ok
test tests::immutable_digest_and_source_are_required ... ok
test tests::readiness_requires_successful_http_response ... ok
test tests::retry_requires_matching_source_and_complete_immutable_metadata ... ok
test tests::owner_workflow_smokes_before_pushing_and_limits_credentials ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Count 13→17 is scoped release lanes (5 policy plus adversary 8→12); before count is the correction report's executed result. Runtime's 42 cases were not repeated. rustfmt on the test file, clippy-driver --edition=2024 --test ci/release_adversary.rs -o "$TMPDIR/adversary-tests" -D warnings, and git diff --check each exited 0.

4. Findings

Nothing found. Both pass-1 findings no longer reproduce: an absent registry package refuses, and uploaded draft recovery completes using the retained receipt despite unrelated locally rebuilt metadata.

5. Attacked and not broken

Malformed or mismatched uploaded receipt refuses both completed and draft recovery.
Moved tag and source outside provider default branch refuse before reuse.
Missing registry artifacts, wrong index membership, and unavailable signatures refuse both release states.
Interrupted draft receipt remains authoritative and recovery does not request native builds.
Workflow now routes verified draft resumes past skipped image jobs and installs registry/signature tools before prepare.
No live registry publication, native architecture image build, or additional runtime smoke was run in this pass; behavior against real provider services remains coordinator verification.

6. Every outside-worktree file written or overwritten in this pass

- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-helper
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-tests
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-policy-tests
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass-2.md
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-package/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-uploaded-draft/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-forbidden-package/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-completed-release/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-false/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-false/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-false/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-false/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-false/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-false/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-false/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-false/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-false/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-false/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-false/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-false/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-false/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-false/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-false/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-false/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-false/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-false/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-false/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-false/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-false/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-false/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-false/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-false/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-false/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-false/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-false/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-false/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-false/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-false/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-true/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-true/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-true/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-true/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-true/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-missing-artifact-true/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-true/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-true/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-true/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-true/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-true/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-wrong-index-true/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-true/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-true/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-true/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-true/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-true/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-unsigned-true/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-true/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-true/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-true/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-true/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-true/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-malformed-true/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-true/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-true/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-true/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-true/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-true/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-source-true/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-moved-tag/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-moved-tag/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-moved-tag/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-moved-tag/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-moved-tag/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-moved-tag/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-unmerged-source/Cargo.toml
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-unmerged-source/output
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-unmerged-source/bin/gh
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-unmerged-source/bin/git
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-unmerged-source/bin/docker
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-pass2-unmerged-source/bin/cosign
- /home/timo/.cache/independent-publication-wave.e1Bll4/workspace/adversary-interrupted-announce/release-manifest.json

```findings
[]
```

