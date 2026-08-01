use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Client, Proxy};

#[derive(Debug, Clone, Copy)]
pub enum ClientProfile {
    Llm,
    Backend,
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientFingerprint {
    pub timeout_secs: u64,
    pub connect_timeout_secs: u64,
    pub proxy: Option<String>,
    pub redirect_limit: usize,
    pub user_agent: Option<String>,
}

impl Default for ClientFingerprint {
    fn default() -> Self {
        Self {
            timeout_secs: 0,
            connect_timeout_secs: 0,
            proxy: None,
            redirect_limit: 10,
            user_agent: None,
        }
    }
}

#[derive(Default)]
struct ClientProfiles {
    llm: RwLock<HashMap<ClientFingerprint, Client>>,
    backend: RwLock<HashMap<ClientFingerprint, Client>>,
    web: RwLock<HashMap<ClientFingerprint, Client>>,
}

static PROFILES: OnceLock<ClientProfiles> = OnceLock::new();

pub fn client(profile: ClientProfile, fingerprint: ClientFingerprint) -> Result<Client> {
    let profiles = PROFILES.get_or_init(ClientProfiles::default);
    let slot = match profile {
        ClientProfile::Llm => &profiles.llm,
        ClientProfile::Backend => &profiles.backend,
        ClientProfile::Web => &profiles.web,
    };
    if let Some(cached) = slot
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .get(&fingerprint)
    {
        return Ok(cached.clone());
    }
    client_in_slot(slot, fingerprint, build)
}

fn client_in_slot(
    slot: &RwLock<HashMap<ClientFingerprint, Client>>,
    fingerprint: ClientFingerprint,
    builder: impl FnOnce(&ClientFingerprint) -> Result<Client>,
) -> Result<Client> {
    let mut guard = slot.write().unwrap_or_else(|error| error.into_inner());
    if let Some(cached) = guard.get(&fingerprint) {
        return Ok(cached.clone());
    }
    let built = builder(&fingerprint)?;
    guard.insert(fingerprint, built.clone());
    Ok(built)
}

pub fn default_client(profile: ClientProfile) -> Client {
    client(profile, ClientFingerprint::default()).unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to build pooled HTTP client; using reqwest default");
        Client::new()
    })
}

fn build(fingerprint: &ClientFingerprint) -> Result<Client> {
    let mut builder = Client::builder().redirect(reqwest::redirect::Policy::limited(
        fingerprint.redirect_limit,
    ));
    if fingerprint.timeout_secs > 0 {
        builder = builder.timeout(Duration::from_secs(fingerprint.timeout_secs));
    }
    if fingerprint.connect_timeout_secs > 0 {
        builder = builder.connect_timeout(Duration::from_secs(fingerprint.connect_timeout_secs));
    }
    if let Some(proxy) = fingerprint.proxy.as_deref() {
        builder = builder.proxy(Proxy::all(proxy).context("invalid HTTP proxy")?);
    }
    if let Some(user_agent) = fingerprint.user_agent.as_deref() {
        builder = builder.user_agent(user_agent);
    }
    builder.build().context("build HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn same_fingerprint_reuses_and_new_fingerprint_replaces() {
        let slot = RwLock::new(HashMap::new());
        let builds = AtomicUsize::new(0);
        let first = ClientFingerprint::default();
        let second = ClientFingerprint {
            timeout_secs: 60,
            ..first.clone()
        };
        let make = |_: &ClientFingerprint| {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok(Client::new())
        };
        client_in_slot(&slot, first.clone(), make).unwrap();
        client_in_slot(&slot, first, make).unwrap();
        client_in_slot(&slot, second.clone(), make).unwrap();
        assert_eq!(builds.load(Ordering::SeqCst), 2);
        assert!(slot.read().unwrap().contains_key(&second));
        assert_eq!(slot.read().unwrap().len(), 2);
    }
}
