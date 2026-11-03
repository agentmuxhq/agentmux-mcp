//! Secrets live in the OS keychain, never in the vault DB or the markdown export.
//!
//! `AGENTMUX_SECRET_<NAME>` overrides the keychain for headless/CI use and tests.

use anyhow::{anyhow, Result};

const SERVICE: &str = "dev.agentmux";

fn env_name(name: &str) -> String {
    format!("AGENTMUX_SECRET_{}", name.to_uppercase().replace(['-', '.', ':'], "_"))
}

pub fn get(name: &str) -> Result<Option<String>> {
    if let Ok(v) = std::env::var(env_name(name)) {
        if !v.is_empty() {
            return Ok(Some(v));
        }
    }
    let entry = keyring::Entry::new(SERVICE, name).map_err(|e| anyhow!("keychain unavailable: {e}"))?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow!("keychain read failed: {e}")),
    }
}

/// When a secret comes from the environment (CI, tests), the keychain is left alone entirely.
fn env_managed(name: &str) -> bool {
    std::env::var_os(env_name(name)).is_some()
}

pub fn set(name: &str, value: &str) -> Result<()> {
    if env_managed(name) {
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE, name).map_err(|e| anyhow!("keychain unavailable: {e}"))?;
    entry.set_password(value).map_err(|e| anyhow!("keychain write failed: {e}"))
}

pub fn delete(name: &str) -> Result<()> {
    if env_managed(name) {
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE, name).map_err(|e| anyhow!("keychain unavailable: {e}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow!("keychain delete failed: {e}")),
    }
}
