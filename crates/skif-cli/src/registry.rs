//! The HTTP side of `skif update` (ADR 0050): the only code in skif that opens a network connection.
//!
//! Blocking `ureq` over rustls (ring), with the operating system's trust store (`SSL_CERT_FILE` and `SSL_CERT_DIR`
//! override it), the proxy of `HTTPS_PROXY` / `ALL_PROXY` and `NO_PROXY`, gzip, a 10 second connect timeout and a 60
//! second limit per request. Requests run on up to `[update] max-concurrent-requests` threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use skif::update::{FetchError, Packument, Registry};
use ureq::Agent;
use ureq::config::RedirectAuthHeaders;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

use crate::npmrc::{Npmrc, package_url};

/// The largest registry document read: the full documents of long-lived packages run to tens of megabytes, and a
/// larger answer is refused rather than held in memory.
const BODY_LIMIT: u64 = 64 * 1024 * 1024;

/// The npm registry over HTTP.
pub struct HttpRegistry {
    agent: Agent,
    npmrc: Npmrc,
    concurrency: usize,
}

impl HttpRegistry {
    pub fn new(npmrc: Npmrc, concurrency: usize) -> Self {
        let roots: Vec<Certificate<'static>> = rustls_native_certs::load_native_certs()
            .certs
            .into_iter()
            .map(|der| Certificate::from_der(der.as_ref()).to_owned())
            .collect();
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::new_with_certs(&roots))
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent: Agent = Agent::config_builder()
            .tls_config(tls)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            // A redirect never carries the token to another host: ureq's default today, stated so an upgrade cannot
            // change it.
            .redirect_auth_headers(RedirectAuthHeaders::Never)
            .user_agent(concat!("skif/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            agent,
            npmrc,
            concurrency: concurrency.max(1),
        }
    }
}

impl Registry for HttpRegistry {
    fn fetch(&self, name: &str) -> Result<Packument, FetchError> {
        let registry = self.npmrc.registry_for(name);
        let url = package_url(&registry, name);
        let mut request = self.agent.get(&url).header("Accept", "application/json");
        if let Some(token) = self.npmrc.token_for(&registry) {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        // The URL holds no credential (see `npmrc`), so it may appear in a message.
        let mut response = request
            .call()
            .map_err(|error| FetchError::Network(format!("{url}: {error}")))?;
        match response.status().as_u16() {
            200 => {}
            404 => return Err(FetchError::NotFound),
            status => return Err(FetchError::Status(status)),
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_string()
            .map_err(|error| FetchError::Network(format!("{url}: {error}")))?;
        Packument::from_json(&text)
    }

    /// Up to `concurrency` requests at once; answers in the order of `names`.
    fn fetch_all(&self, names: &[&str]) -> Vec<Result<Packument, FetchError>> {
        let next = AtomicUsize::new(0);
        let mut answers: Vec<Option<Result<Packument, FetchError>>> = Vec::new();
        answers.resize_with(names.len(), || None);
        let slots = std::sync::Mutex::new(answers);
        std::thread::scope(|scope| {
            for _ in 0..self.concurrency.min(names.len()) {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(name) = names.get(index) else {
                            break;
                        };
                        let answer = self.fetch(name);
                        if let Ok(mut slots) = slots.lock() {
                            slots[index] = Some(answer);
                        }
                    }
                });
            }
        });
        slots
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .into_iter()
            .map(|answer| answer.unwrap_or_else(|| Err(FetchError::Network("the request did not run".to_owned()))))
            .collect()
    }
}
