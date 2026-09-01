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
