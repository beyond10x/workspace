use std::{collections::BTreeMap, time::Duration};

use aep_client::wire::{Method, Request, Response};
use aep_client::{
    BearerToken, ClientConfigurationError, CredentialError, CredentialProvider, Transport,
    TransportError,
};
use reqwest::header::{HeaderName, HeaderValue};
use url::Url;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct AepTransport {
    origin: Url,
    http: reqwest::Client,
}

impl AepTransport {
    pub(crate) fn new(origin: &str) -> Result<Self, ClientConfigurationError> {
        let origin = Url::parse(origin).map_err(|_| configuration())?;
        let internal_http = origin.scheme() == "http"
            && origin.host_str().is_some_and(|host| {
                host == "127.0.0.1" || host == "localhost" || host.ends_with(".svc.cluster.local")
            });
        if !(origin.scheme() == "https" || internal_http)
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(configuration());
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| configuration())?;
        Ok(Self { origin, http })
    }
}

fn configuration() -> ClientConfigurationError {
    ClientConfigurationError::Coordinate {
        kind: "origin",
        reason: "must be an exact HTTPS or internal-cluster HTTP origin".to_owned(),
    }
}

impl Transport for AepTransport {
    async fn send(&self, request: Request) -> Result<Response, TransportError> {
        let endpoint = self
            .origin
            .join(request.path.trim_start_matches('/'))
            .map_err(|_| TransportError::new("AEP request path is invalid"))?;
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut exchange = self.http.request(method, endpoint);
        for (name, value) in request.headers {
            let name = HeaderName::try_from(name)
                .map_err(|_| TransportError::new("AEP request header is invalid"))?;
            let value = HeaderValue::try_from(value)
                .map_err(|_| TransportError::new("AEP request header is invalid"))?;
            exchange = exchange.header(name, value);
        }
        if !request.body.is_empty() {
            exchange = exchange.body(request.body);
        }
        let response = exchange
            .send()
            .await
            .map_err(|_| TransportError::new("AEP service did not answer"))?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(TransportError::new("AEP response exceeded its bound"));
        }
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.to_string(), value.to_owned()))
            })
            .collect::<BTreeMap<_, _>>();
        let body = response
            .bytes()
            .await
            .map_err(|_| TransportError::new("AEP response body was unavailable"))?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(TransportError::new("AEP response exceeded its bound"));
        }
        Ok(Response {
            status,
            headers,
            body: body.to_vec(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct RequestCredential(String);

impl RequestCredential {
    pub(crate) fn from_authorization(value: &str) -> Result<Self, CredentialError> {
        value
            .strip_prefix("Bearer ")
            .filter(|token| !token.is_empty())
            .map(|token| Self(token.to_owned()))
            .ok_or_else(|| CredentialError::unauthenticated("an Identity session is required"))
    }
}

impl std::fmt::Debug for RequestCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RequestCredential([REDACTED])")
    }
}

impl CredentialProvider for RequestCredential {
    async fn credential(&self) -> Result<BearerToken, CredentialError> {
        BearerToken::new(self.0.clone())
            .map_err(|_| CredentialError::unauthenticated("the Identity session is malformed"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The released AEP contract graph this repository compiles against. Both declared pins and
    /// every crate the lockfile resolves out of the AEP repository state exactly this release, so
    /// a partial re-pin cannot leave Workspace compiling two AEP contract graphs at once.
    const AEP_RELEASE: &str = "0.51.0";
    const AEP_REPOSITORY: &str = "https://github.com/beyond10x/aep";

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the service crate sits two directories below the workspace root")
            .to_path_buf()
    }

    fn workspace_file(name: &str) -> String {
        let path = workspace_root().join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
    }

    /// The first `<key> = "<value>"` in `text`, wherever it sits: `Cargo.lock` writes one field
    /// per line and `Cargo.toml` writes a dependency's keys inline in one table.
    fn quoted_value(text: &str, key: &str) -> Option<String> {
        text.split_once(&format!("{key} = \""))?
            .1
            .split('"')
            .next()
            .map(str::to_owned)
    }

    fn declared_tag(manifest: &str, crate_name: &str) -> String {
        let prefix = format!("{crate_name} = ");
        let line = manifest
            .lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_else(|| panic!("{crate_name} is declared in [workspace.dependencies]"));
        assert!(
            line.contains(&format!("git = \"{AEP_REPOSITORY}\"")),
            "{crate_name} is pinned to the AEP repository: {line}"
        );
        quoted_value(line, "tag").unwrap_or_else(|| panic!("{crate_name} pins a tag: {line}"))
    }

    #[test]
    fn declared_aep_pins_name_the_released_contract_graph() {
        let manifest = workspace_file("Cargo.toml");
        for crate_name in ["aep-client", "aep-contract"] {
            assert_eq!(
                declared_tag(&manifest, crate_name),
                AEP_RELEASE,
                "{crate_name} declares the released AEP tag"
            );
        }
    }

    #[test]
    fn every_locked_aep_crate_resolves_from_that_one_release() {
        let lock = workspace_file("Cargo.lock");
        let source_prefix = format!("git+{AEP_REPOSITORY}?");
        let mut resolved = Vec::new();
        for block in lock.split("[[package]]").skip(1) {
            let Some(source) = quoted_value(block, "source") else {
                continue;
            };
            if !source.starts_with(&source_prefix) {
                continue;
            }
            let name = quoted_value(block, "name").expect("a locked package states its name");
            assert!(
                source.contains(&format!("?tag={AEP_RELEASE}#")),
                "{name} resolves from the released AEP tag: {source}"
            );
            assert_eq!(
                quoted_value(block, "version").expect("a locked package states its version"),
                AEP_RELEASE,
                "{name} resolves at the released AEP version"
            );
            resolved.push(name);
        }
        for crate_name in ["aep-client", "aep-contract"] {
            assert_eq!(
                resolved.iter().filter(|name| *name == crate_name).count(),
                1,
                "the lockfile resolves exactly one {crate_name} from the AEP repository: {resolved:?}"
            );
        }
    }
}
