use std::{env, fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

fn probe(name: &str, visibility: &str, release_state: &str) -> std::process::Output {
    let root = PathBuf::from(env::var("TMPDIR").unwrap()).join(name);
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace.package]\nversion = \"0.2.17\"\n",
    )
    .unwrap();
    fs::write(root.join("output"), "").unwrap();
    let gh = r#"#!/bin/sh
case "$*" in
  *'/packages/container/'*)
    case "$PROBE_VISIBILITY" in
      missing) echo 'gh: Not Found (HTTP 404)' >&2; exit 1;;
      forbidden) echo 'gh: Forbidden (HTTP 403)' >&2; exit 1;;
      *) echo private;;
    esac;;
  *'/packages?'*) exit 0;;
  *'/releases?'*) printf '0.2.17\t%s\n' "$PROBE_RELEASE_STATE";;
  'release view '*) echo release-manifest.json;;
  'release edit '*) exit 0;;
  'release download '*) printf '{"version":"0.2.17","source_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","artifacts":{"workspace_service":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"platforms":{"linux/amd64":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","linux/arm64":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}}\n';;
  *'/repos/'*) echo trunk;;
  *) echo 'unexpected mock invocation' >&2; exit 7;;
esac
"#;
    let git = "#!/bin/sh\ncase \"$1\" in\nfetch|merge-base) exit 0;;\nrev-parse) echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;;\n*) exit 7;;\nesac\n";
    let docker = r#"#!/bin/sh
if [ "$PROBE_VISIBILITY" = artifact-missing ]; then exit 1; fi
case "$*" in
  *--raw*)
    if [ "$PROBE_VISIBILITY" = wrong-index ]; then echo '{"manifests":[]}'; exit 0; fi
    printf '{"manifests":[{"platform":{"os":"linux","architecture":"amd64"},"digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},{"platform":{"os":"linux","architecture":"arm64"},"digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}]}';;
  *) printf '%s\n' "${4#*@}";;
esac
"#;
    let cosign =
        "#!/bin/sh\nif [ \"$PROBE_VISIBILITY\" = unsigned ]; then exit 1; fi\necho verified\n";
    for (name, body) in [
        ("gh", gh),
        ("git", git),
        ("docker", docker),
        ("cosign", cosign),
    ] {
        let file = root.join("bin").join(name);
        fs::write(&file, body).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    Command::new(env::var("ADV_HELPER").unwrap())
        .arg("prepare")
        .current_dir(&root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                env::var("PATH").unwrap()
            ),
        )
        .env("GITHUB_REPOSITORY", "example/workspace")
        .env("WORKSPACE_IMAGE", "ghcr.io/example/workspace")
        .env("RELEASE_VERSION", "0.2.17")
        .env("SOURCE_COMMIT", "a".repeat(40))
        .env("GITHUB_OUTPUT", root.join("output"))
        .env("PROBE_VISIBILITY", visibility)
        .env("PROBE_RELEASE_STATE", release_state)
        .output()
        .unwrap()
}

fn output(name: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env::var("TMPDIR").unwrap())
            .join(name)
            .join("output"),
    )
    .unwrap()
}

#[test]
fn successful_release_with_deleted_registry_package_is_not_a_completed_retry() {
    let result = probe("adversary-missing-package", "missing", "false");
    assert!(
        !result.status.success(),
        "deleted package was accepted as completed: {}",
        output("adversary-missing-package")
    );
}

#[test]
fn draft_with_uploaded_immutable_manifest_does_not_restart_image_builds() {
    let result = probe("adversary-uploaded-draft", "private", "true");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !output("adversary-uploaded-draft").contains("published=false"),
        "draft already has an uploaded immutable manifest but prepare unconditionally restarts both native builds"
    );
}

#[test]
fn forbidden_registry_is_not_bootstrap() {
    let result = probe("adversary-forbidden-package", "forbidden", "false");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("cannot verify private package"));
}

#[test]
fn matching_completed_release_skips_builds_with_provider_default_branch() {
    let result = probe("adversary-completed-release", "private", "false");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let fields = output("adversary-completed-release");
    assert!(fields.contains("published=true"));
    assert!(fields.contains("default_branch=trunk"));
}

#[test]
fn retry_after_manifest_upload_and_edit_failure_can_finish() {
    let name = "adversary-interrupted-announce";
    let prepared = probe(name, "private", "true");
    assert!(prepared.status.success());
    let root = PathBuf::from(env::var("TMPDIR").unwrap()).join(name);
    // A rerun receives fresh image receipts after prepare requests native builds again.
    let manifest = format!(
        "{{\"version\":\"0.2.17\",\"source_commit\":\"{}\",\"artifacts\":{{\"workspace_service\":\"sha256:{}\"}},\"platforms\":{{\"linux/amd64\":\"sha256:{}\",\"linux/arm64\":\"sha256:{}\"}}}}",
        "a".repeat(40),
        "e".repeat(64),
        "f".repeat(64),
        "1".repeat(64)
    );
    fs::write(root.join("release-manifest.json"), manifest).unwrap();
    let result = Command::new(env::var("ADV_HELPER").unwrap())
        .arg("announce")
        .current_dir(&root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                env::var("PATH").unwrap()
            ),
        )
        .env("GITHUB_REPOSITORY", "example/workspace")
        .env("RELEASE_VERSION", "0.2.17")
        .env("SOURCE_COMMIT", "a".repeat(40))
        .env("WORKSPACE_IMAGE", "ghcr.io/example/workspace")
        .env("PROBE_RELEASE_STATE", "true")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "retry cannot finish uploaded draft: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn completed_and_draft_receipts_refuse_unavailable_artifacts() {
    for state in ["false", "true"] {
        let result = probe(
            &format!("adversary-missing-artifact-{state}"),
            "artifact-missing",
            state,
        );
        assert!(
            !result.status.success(),
            "receipt accepted a missing image artifact"
        );
    }
}

#[test]
fn completed_and_draft_receipts_refuse_wrong_index_membership() {
    for state in ["false", "true"] {
        let result = probe(
            &format!("adversary-wrong-index-{state}"),
            "wrong-index",
            state,
        );
        assert!(
            !result.status.success(),
            "receipt accepted unrelated platform digests"
        );
    }
}

#[test]
fn completed_and_draft_receipts_require_owner_signature() {
    for state in ["false", "true"] {
        let result = probe(&format!("adversary-unsigned-{state}"), "unsigned", state);
        assert!(
            !result.status.success(),
            "receipt accepted an unsigned image"
        );
    }
}

fn replay_prepare(name: &str, state: &str) -> std::process::Output {
    let root = PathBuf::from(env::var("TMPDIR").unwrap()).join(name);
    Command::new(env::var("ADV_HELPER").unwrap())
        .arg("prepare")
        .current_dir(&root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                env::var("PATH").unwrap()
            ),
        )
        .env("GITHUB_REPOSITORY", "example/workspace")
        .env("WORKSPACE_IMAGE", "ghcr.io/example/workspace")
        .env("RELEASE_VERSION", "0.2.17")
        .env("SOURCE_COMMIT", "a".repeat(40))
        .env("GITHUB_OUTPUT", root.join("output"))
        .env("PROBE_VISIBILITY", "private")
        .env("PROBE_RELEASE_STATE", state)
        .output()
        .unwrap()
}

#[test]
fn malformed_uploaded_receipt_refuses_completed_and_draft_recovery() {
    for state in ["false", "true"] {
        let name = format!("adversary-pass2-malformed-{state}");
        assert!(probe(&name, "private", state).status.success());
        let file = PathBuf::from(env::var("TMPDIR").unwrap())
            .join(&name)
            .join("bin/gh");
        let original = fs::read_to_string(&file).unwrap();
        fs::write(
            &file,
            original.replace(
                "'release download '*) printf '",
                "'release download '*) printf 'truncated-",
            ),
        )
        .unwrap();
        let result = replay_prepare(&name, state);
        assert!(!result.status.success(), "malformed receipt accepted");
        assert!(String::from_utf8_lossy(&result.stderr).contains("not valid JSON"));
    }
}

#[test]
fn different_receipt_source_refuses_completed_and_draft_recovery() {
    for state in ["false", "true"] {
        let name = format!("adversary-pass2-source-{state}");
        assert!(probe(&name, "private", state).status.success());
        let file = PathBuf::from(env::var("TMPDIR").unwrap())
            .join(&name)
            .join("bin/gh");
        let original = fs::read_to_string(&file).unwrap();
        fs::write(&file, original.replace(&"a".repeat(40), &"e".repeat(40))).unwrap();
        let result = replay_prepare(&name, state);
        assert!(
            !result.status.success(),
            "receipt for another source accepted"
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("stored release identity conflicts")
        );
    }
}

#[test]
fn moved_tag_refuses_before_completed_receipt_can_skip_builds() {
    let name = "adversary-pass2-moved-tag";
    assert!(probe(name, "private", "false").status.success());
    let file = PathBuf::from(env::var("TMPDIR").unwrap())
        .join(name)
        .join("bin/git");
    let original = fs::read_to_string(&file).unwrap();
    fs::write(&file, original.replace(&"a".repeat(40), &"e".repeat(40))).unwrap();
    let result = replay_prepare(name, "false");
    assert!(!result.status.success(), "changed tag accepted");
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("release tag does not name requested source")
    );
}

#[test]
fn source_outside_default_branch_refuses_before_receipt_reuse() {
    let name = "adversary-pass2-unmerged-source";
    assert!(probe(name, "private", "false").status.success());
    let file = PathBuf::from(env::var("TMPDIR").unwrap())
        .join(name)
        .join("bin/git");
    let original = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        original.replace(
            "fetch|merge-base) exit 0;;",
            "fetch) exit 0;;\nmerge-base) echo not-ancestor >&2; exit 1;;",
        ),
    )
    .unwrap();
    let result = replay_prepare(name, "false");
    assert!(!result.status.success(), "unmerged source accepted");
    assert!(String::from_utf8_lossy(&result.stderr).contains("not-ancestor"));
}
