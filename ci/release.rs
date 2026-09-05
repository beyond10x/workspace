//! Dependency-free release policy and container readiness probe.
use std::{
    env, fs,
    io::{Read, Write},
    net::TcpStream,
    process::{Command, Stdio},
    time::Duration,
};

fn version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}
fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| hex(v, 64))
}
fn source(value: &str) -> bool {
    hex(value, 40)
}
fn http_ready(value: &str) -> bool {
    value.starts_with("HTTP/1.1 200 ")
        && (value.contains("\"status\":\"ready\"") || value.contains("\"status\":\"ok\""))
}
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn required(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("missing {name}"))
}
fn existing_identity(fields: &str, release: &str, commit: &str) -> bool {
    let values: Vec<_> = fields.lines().collect();
    values.len() == 5
        && values[0] == release
        && values[1] == commit
        && values[2..].iter().all(|value| digest(value))
}
fn json_fields(json: &str) -> Result<String, String> {
    query_json(
        json,
        ".version, .source_commit, .artifacts.workspace_service, .platforms[\"linux/amd64\"], .platforms[\"linux/arm64\"]",
    )
}
fn query_json(json: &str, query: &str) -> Result<String, String> {
    let mut child = Command::new("jq")
        .args(["-r", query])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("jq stdin unavailable")?
        .write_all(json.as_bytes())
        .map_err(|e| e.to_string())?;
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    check(
        result.status.success(),
        "existing release manifest is not valid JSON",
    )?;
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}
fn receipt(release: &str, commit: &str) -> Result<String, String> {
    private_package(false)?;
    let json = run(
        "gh",
        &[
            "release",
            "download",
            release,
            "--pattern",
            "release-manifest.json",
            "--output",
            "-",
        ],
    )?;
    let fields = json_fields(&json)?;
    check(
        existing_identity(&fields, release, commit),
        "stored release identity conflicts with requested source or lacks immutable platform metadata",
    )?;
    let values: Vec<_> = fields.lines().collect();
    let image = required("WORKSPACE_IMAGE")?;
    // Verify every recorded digest still exists, not only its package or release asset.
    for digest in &values[2..] {
        let observed = run(
            "docker",
            &[
                "buildx",
                "imagetools",
                "inspect",
                &format!("{image}@{digest}"),
                "--format",
                "{{.Manifest.Digest}}",
            ],
        )?;
        check(
            observed == *digest,
            "stored registry artifact is unavailable or has a different digest",
        )?;
    }
    let index = run(
        "docker",
        &[
            "buildx",
            "imagetools",
            "inspect",
            &format!("{image}@{}", values[2]),
            "--raw",
        ],
    )?;
    let platforms = query_json(
        &index,
        ".manifests[] | [.platform.os + \"/\" + .platform.architecture, .digest] | @tsv",
    )?;
    for (platform, expected) in [("linux/amd64", values[3]), ("linux/arm64", values[4])] {
        check(
            platforms
                .lines()
                .any(|line| line == format!("{platform}\t{expected}")),
            "stored platform receipt does not belong to its image index",
        )?;
    }
    let repository = required("GITHUB_REPOSITORY")?;
    let default_branch = run(
        "gh",
        &[
            "api",
            &format!("/repos/{repository}"),
            "--jq",
            ".default_branch",
        ],
    )?;
    let verify = |reference: &str| {
        run(
            "cosign",
            &[
                "verify",
                &format!("{image}@{}", values[2]),
                "--certificate-identity",
                &format!(
                    "https://github.com/{repository}/.github/workflows/release.yml@{reference}"
                ),
                "--certificate-oidc-issuer",
                "https://token.actions.githubusercontent.com",
            ],
        )
    };
    verify(&format!("refs/tags/{release}"))
        .or_else(|_| verify(&format!("refs/heads/{default_branch}")))?;
    Ok(json)
}
fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}
fn output(key: &str, value: &str) -> Result<(), String> {
    check(!value.contains(['\r', '\n']), "multiline output refused")?;
    let path = required("GITHUB_OUTPUT")?;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{key}={value}").map_err(|e| e.to_string())
}
fn private_package(allow_missing: bool) -> Result<(), String> {
    let image = required("WORKSPACE_IMAGE")?;
    let repository = required("GITHUB_REPOSITORY")?;
    let owner = repository.split('/').next().ok_or("missing owner")?;
    // The package target is derived from the image, so one package cannot authorize another.
    let name = image
        .strip_prefix(&format!("ghcr.io/{}/", owner.to_ascii_lowercase()))
        .ok_or("image must use this owner's GHCR namespace")?;
    check(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_./".contains(&b)),
        "invalid image name",
    )?;
    let api = format!(
        "/orgs/{owner}/packages/container/{}",
        name.replace('/', "%2F")
    );
    let result = Command::new("gh")
        .args(["api", &api, "--jq", ".visibility"])
        .output()
        .map_err(|e| e.to_string())?;
    if result.status.success() {
        return check(
            String::from_utf8_lossy(&result.stdout).trim() == "private",
            "target package is not private",
        );
    }
    // GHCR's documented initial visibility is private. Confirm absence with an authenticated
    // list request, never treating an arbitrary API failure as absence.
    // https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
    if allow_missing && String::from_utf8_lossy(&result.stderr).contains("(HTTP 404)") {
        let names = run(
            "gh",
            &[
                "api",
                &format!("/orgs/{owner}/packages?package_type=container&per_page=100"),
                "--paginate",
                "--jq",
                ".[].name",
            ],
        )?;
        check(
            !names.lines().any(|n| n == name),
            "package exists but cannot be inspected",
        )?;
        return Ok(());
    }
    Err("cannot verify private package".into())
}
fn prepare() -> Result<(), String> {
    let repository = required("GITHUB_REPOSITORY")?;
    let release = required("RELEASE_VERSION")?;
    let commit = required("SOURCE_COMMIT")?;
    check(
        version(&release) && source(&commit),
        "exact semantic version and 40-character source commit required",
    )?;
    let default_branch = run(
        "gh",
        &[
            "api",
            &format!("/repos/{repository}"),
            "--jq",
            ".default_branch",
        ],
    )?;
    check(
        !default_branch.is_empty() && !default_branch.contains(['\n', '\r']),
        "invalid default branch",
    )?;
    run(
        "git",
        &[
            "fetch",
            "--no-tags",
            "origin",
            &format!("refs/heads/{default_branch}"),
        ],
    )?;
    run(
        "git",
        &["merge-base", "--is-ancestor", &commit, "FETCH_HEAD"],
    )?;
    let tag_commit = run(
        "git",
        &["rev-parse", &format!("refs/tags/{release}^{{commit}}")],
    )?;
    check(
        tag_commit == commit,
        "release tag does not name requested source",
    )?;
    check(
        run("git", &["rev-parse", "HEAD"])? == commit,
        "checkout is not requested source",
    )?;
    let manifest = fs::read_to_string("Cargo.toml").map_err(|e| e.to_string())?;
    let declared = manifest
        .split("[workspace.package]")
        .nth(1)
        .ok_or("workspace package missing")?
        .split("\n[")
        .next()
        .unwrap_or_default()
        .lines()
        .find_map(|l| {
            l.strip_prefix("version = \"")
                .and_then(|v| v.strip_suffix('"'))
        })
        .ok_or("version missing")?;
    check(
        declared == release,
        "source package version does not match release",
    )?;
    let releases = run(
        "gh",
        &[
            "api",
            &format!("/repos/{repository}/releases?per_page=100"),
            "--paginate",
            "--jq",
            ".[] | [.tag_name, .draft] | @tsv",
        ],
    )?;
    let completed = releases
        .lines()
        .any(|line| line == format!("{release}\tfalse"));
    let draft = releases
        .lines()
        .any(|line| line == format!("{release}\ttrue"));
    private_package(!completed && !draft)?;
    let has_receipt = if completed || draft {
        run(
            "gh",
            &[
                "release",
                "view",
                &release,
                "--json",
                "assets",
                "--jq",
                ".assets[].name",
            ],
        )?
        .lines()
        .any(|name| name == "release-manifest.json")
    } else {
        false
    };
    check(
        !completed || has_receipt,
        "published release has no immutable receipt",
    )?;
    if has_receipt {
        receipt(&release, &commit)?;
        output("published", if completed { "true" } else { "draft" })?;
        output("build", "false")?;
        output("resume", if draft { "true" } else { "false" })?;
    } else {
        output("published", "false")?;
        output("build", "true")?;
        output("resume", "false")?;
    }
    output("version", &release)?;
    output("source", &commit)?;
    output("default_branch", &default_branch)
}
struct Container(String);
impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}
fn smoke(image: &str) -> Result<(), String> {
    let id = run(
        "docker",
        &[
            "run",
            "--detach",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--tmpfs",
            "/data:rw,noexec,nosuid,size=16m,uid=65532,gid=65532",
            "--publish",
            "127.0.0.1::8094",
            "--env",
            "WORKSPACE_LISTEN=0.0.0.0:8094",
            "--env",
            "WORKSPACE_DATABASE_URL=sqlite:///data/smoke.sqlite?mode=rwc",
            "--env",
            "WORKSPACE_IDENTITY_ORIGIN=http://synthetic-identity.svc.cluster.local",
            "--env",
            "WORKSPACE_CONNECTORS_API_BASE=http://synthetic-connectors.svc.cluster.local",
            image,
        ],
    )?;
    let container = Container(id);
    let address = run("docker", &["port", &container.0, "8094/tcp"])?;
    for path in ["/healthz", "/readyz"] {
        let mut ready = false;
        for _ in 0..60 {
            if let Ok(mut socket) = TcpStream::connect(&address) {
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .map_err(|e| e.to_string())?;
                socket
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .map_err(|e| e.to_string())?;
                let _ = write!(
                    socket,
                    "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                );
                let mut response = String::new();
                if socket.read_to_string(&mut response).is_ok() && http_ready(&response) {
                    ready = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if !ready {
            let logs = run("docker", &["logs", &container.0]).unwrap_or_default();
            return Err(format!("image failed {path} readiness probe: {logs}"));
        }
        println!("{path}: HTTP 200, runtime ready");
    }
    Ok(())
}
fn manifest() -> Result<(), String> {
    let version = required("RELEASE_VERSION")?;
    let commit = required("SOURCE_COMMIT")?;
    let image = required("IMAGE_DIGEST")?;
    let amd64 = required("AMD64_DIGEST")?;
    let arm64 = required("ARM64_DIGEST")?;
    check(
        self::version(&version)
            && source(&commit)
            && [&image, &amd64, &arm64].into_iter().all(|v| digest(v)),
        "invalid release identity",
    )?;
    // No private registry coordinate is written to public metadata.
    let json = format!(
        "{{\"schema\":1,\"version\":\"{version}\",\"source_commit\":\"{commit}\",\"artifacts\":{{\"workspace_service\":\"{image}\"}},\"platforms\":{{\"linux/amd64\":\"{amd64}\",\"linux/arm64\":\"{arm64}\"}}}}\n"
    );
    fs::write("release-manifest.json", json).map_err(|e| e.to_string())
}
fn announce() -> Result<(), String> {
    let release = required("RELEASE_VERSION")?;
    check(version(&release), "invalid version")?;
    let repository = required("GITHUB_REPOSITORY")?;
    let releases = run(
        "gh",
        &[
            "api",
            &format!("/repos/{repository}/releases?per_page=100"),
            "--paginate",
            "--jq",
            ".[] | [.tag_name, .draft] | @tsv",
        ],
    )?;
    check(
        !releases
            .lines()
            .any(|line| line == format!("{release}\tfalse")),
        "published release identity is immutable",
    )?;
    if !releases
        .lines()
        .any(|line| line == format!("{release}\ttrue"))
    {
        run(
            "gh",
            &[
                "release",
                "create",
                &release,
                "--draft",
                "--verify-tag",
                "--title",
                &format!("Workspace {release}"),
                "--generate-notes",
            ],
        )?;
    }
    let assets = run(
        "gh",
        &[
            "release",
            "view",
            &release,
            "--json",
            "assets",
            "--jq",
            ".assets[].name",
        ],
    )?;
    if assets.lines().any(|name| name == "release-manifest.json") {
        // The uploaded verified receipt is authoritative. Never replace it with a rebuilt image.
        receipt(&release, &required("SOURCE_COMMIT")?)?;
    } else {
        run(
            "gh",
            &["release", "upload", &release, "release-manifest.json"],
        )?;
    }
    run("gh", &["release", "edit", &release, "--draft=false"])?;
    Ok(())
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("prepare") => prepare(),
        Some("private") => private_package(true),
        Some("private-existing") => private_package(false),
        Some("smoke") => args
            .get(2)
            .ok_or_else(|| "missing image".to_owned())
            .and_then(|image| smoke(image)),
        Some("manifest") => manifest(),
        Some("announce") => announce(),
        _ => Err(
            "expected prepare, private, private-existing, smoke IMAGE, manifest, or announce"
                .into(),
        ),
    };
    if let Err(error) = result {
        eprintln!("release refused: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_version_excludes_injection_and_floating_refs() {
        assert!(version("0.2.17"));
        for value in [
            "main",
            "v0.2.17",
            "0.2.17/evil",
            "0.2.17\n",
            "01.2.17",
            "0.2",
        ] {
            assert!(!version(value));
        }
    }
    #[test]
    fn immutable_digest_and_source_are_required() {
        assert!(digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(source(&"b".repeat(40)));
        assert!(!digest("latest"));
        assert!(!source("main"));
        assert!(!digest(&format!("sha256:{}", "g".repeat(64))));
    }
    #[test]
    fn readiness_requires_successful_http_response() {
        assert!(http_ready(
            "HTTP/1.1 200 OK\r\ncontent-length: 18\r\n\r\n{\"status\":\"ready\"}"
        ));
        assert!(!http_ready("HTTP/1.1 503 Service Unavailable\r\n\r\n{}"));
        assert!(!http_ready("HTTP/1.1 200 OK\r\n\r\n{}"));
    }
    #[test]
    fn retry_requires_matching_source_and_complete_immutable_metadata() {
        let commit = "a".repeat(40);
        let image = format!("sha256:{}", "b".repeat(64));
        let fields = format!("0.2.17\n{commit}\n{image}\n{image}\n{image}\n");
        assert!(existing_identity(&fields, "0.2.17", &commit));
        assert!(!existing_identity(&fields, "0.2.18", &commit));
        assert!(!existing_identity(&fields, "0.2.17", &"c".repeat(40)));
        assert!(!existing_identity(
            &format!("0.2.17\n{commit}\n{image}"),
            "0.2.17",
            &commit
        ));
    }
    #[test]
    fn owner_workflow_smokes_before_pushing_and_limits_credentials() {
        let workflow = fs::read_to_string(".github/workflows/release.yml").expect("owner workflow");
        assert!(
            workflow.find("release-helper smoke").unwrap() < workflow.find("docker push").unwrap()
        );
        assert!(workflow.contains("pull_request:"));
        assert!(workflow.contains("ubuntu-24.04-arm"));
        assert!(workflow.contains("cosign verify"));
        assert!(!workflow.contains("owner: beyond10x\n          repositories: '*'"));
        assert!(!workflow.contains("devcenter:"));
    }
}
