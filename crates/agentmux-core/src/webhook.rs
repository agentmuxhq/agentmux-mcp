//! Grok Bot waker.
//!
//! Grok Bot "routines" can be triggered by a webhook:
//!   POST {backend}/automations/webhook/{routineId}
//!   authorization: Bearer <key>, body {"context": "<string>"}  ->  {"runUuid": "..."}
//! The request only *starts* a run; it never returns the answer. The routine answers by
//! calling AgentMux's `mux_reply` MCP tool with the ticket id carried in the context.

use crate::{secrets, vault::Vault};
use anyhow::{anyhow, bail, Result};
use reqwest::Url;
use serde_json::{json, Value};
use std::time::Duration;

/// The original bot (`grok`) keeps the names used before multi-bot support.
fn url_setting(agent: &str) -> String {
    if agent == "grok" { "grok.webhook_url".into() } else { format!("webhook.{agent}.url") }
}

fn secret_name(agent: &str) -> String {
    match agent.strip_prefix("grok:") {
        Some(slug) => format!("grok-webhook-key:{slug}"),
        None => "grok-webhook-key".into(),
    }
}

fn ensure_bot(agent: &str) -> Result<()> {
    if !crate::config::is_grok_family(agent) {
        bail!("'{agent}' is not a Grok bot");
    }
    Ok(())
}
const PATH_PREFIX: &str = "/automations/webhook/";
const MAX_CONTEXT_DIFF: usize = 6_000;

/// The text a user pastes into the Grok routine's instructions for one bot.
pub fn instructions(agent: &str) -> String {
    format!(
        "You are a Grok bot connected to other AI agents (Cursor, Claude Code, ChatGPT, other bots) through AgentMux, as `{agent}`. \
If the AgentMux tool `mux_whoami` shows `registered: false`, call `mux_register` once with your bot name and a one-line description of what you do. \
Each run's webhook context contains an AgentMux ticket id and a question from another agent. \
Answer it using your own knowledge and tools, then call the AgentMux MCP tool `mux_reply` with that ticket id and your answer as `text`. \
The asker only receives what you send through `mux_reply`, so do not answer only in chat. \
If the AgentMux tools are not available in this run, say so in your final message."
    )
}

/// https only, except loopback (for local testing); must be a routine-webhook path.
pub fn validate_url(raw: &str) -> Result<Url> {
    let u = Url::parse(raw.trim()).map_err(|e| anyhow!("not a valid URL: {e}"))?;
    let loopback = matches!(u.host_str(), Some("localhost") | Some("127.0.0.1") | Some("[::1]"));
    match u.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => bail!("webhook URL must be https (the key is sent as a bearer token)"),
    }
    if u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() {
        bail!("webhook URL must not contain credentials");
    }
    let id = u.path().strip_prefix(PATH_PREFIX).unwrap_or("");
    if id.is_empty() || id.contains('/') {
        bail!("expected a URL like https://<host>/automations/webhook/<routine-id> (copy it from the routine's Webhook trigger in Grok Bot)");
    }
    Ok(u)
}

/// Store URL (vault) and key (keychain), and make Grok wake via webhook.
pub fn save(vault: &Vault, agent: &str, url: &str, key: &str) -> Result<()> {
    ensure_bot(agent)?;
    let u = validate_url(url)?;
    let key = key.trim();
    if key.is_empty() {
        bail!("the webhook key is empty");
    }
    secrets::set(&secret_name(agent), key)?;
    vault.set_setting(&url_setting(agent), u.as_str())?;
    vault.set_wake_kind(agent, "webhook")?;
    Ok(())
}

pub fn clear(vault: &Vault, agent: &str) -> Result<()> {
    ensure_bot(agent)?;
    secrets::delete(&secret_name(agent))?;
    vault.del_setting(&url_setting(agent))?;
    // The row may already be gone when a bot is being removed.
    let _ = vault.set_wake_kind(agent, "inbox");
    Ok(())
}

/// Never returns the key; the URL is shown with the routine id masked.
pub fn status(vault: &Vault, agent: &str) -> Value {
    let url = vault.setting(&url_setting(agent)).ok().flatten();
    let masked = url.as_deref().and_then(|u| Url::parse(u).ok()).map(|u| {
        let id = u.path().strip_prefix(PATH_PREFIX).unwrap_or("");
        let tail: String = id.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        format!("{}://{}{}…{}", u.scheme(), u.host_str().unwrap_or(""), PATH_PREFIX, tail)
    });
    let has_key = secrets::get(&secret_name(agent)).ok().flatten().is_some();
    json!({"agent": agent, "configured": url.is_some() && has_key, "url": masked, "hasKey": has_key, "instructions": instructions(agent)})
}

pub fn load(vault: &Vault, agent: &str) -> Result<(String, String)> {
    let url = vault.setting(&url_setting(agent))?.ok_or_else(|| anyhow!("@{agent} has no webhook URL configured"))?;
    let key = secrets::get(&secret_name(agent))?.ok_or_else(|| anyhow!("@{agent} has no webhook key in the keychain"))?;
    Ok((url, key))
}

/// Fire the routine. Returns the run id when the server reports one.
pub async fn post(url: &str, key: &str, context: &str) -> Result<Option<String>> {
    let u = validate_url(url)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none()) // never forward the bearer key to another host
        .timeout(Duration::from_secs(20))
        .build()?;
    let resp = client
        .post(u)
        .bearer_auth(key)
        .json(&json!({ "context": context }))
        .send()
        .await
        .map_err(|e| anyhow!("could not reach the Grok webhook: {}", e.without_url()))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = body["error"].as_str().map(|s| s.chars().take(300).collect::<String>());
        bail!("Grok webhook rejected the request (HTTP {}){}", status.as_u16(), msg.map(|m| format!(": {m}")).unwrap_or_default());
    }
    Ok(body["runUuid"].as_str().map(String::from))
}

/// The `context` string for a ticket: machine-readable header, question, and shared git diff.
pub fn build_context(ticket: &Value, room: &Value, diff: &str, notes: &str) -> String {
    let mut c = format!(
        "agentmux_ticket={tid}\nFor: @{to}\nFrom: @{from}\nRoom: {title} ({ws}, branch {branch})\n\nQuestion from @{from}:\n{q}\n",
        tid = ticket["id"].as_str().unwrap_or(""),
        to = ticket["to"].as_str().unwrap_or(""),
        from = ticket["from"].as_str().unwrap_or(""),
        title = room["title"].as_str().unwrap_or(""),
        ws = room["workspacePath"].as_str().unwrap_or(""),
        branch = room["branch"].as_str().unwrap_or(""),
        q = ticket["question"].as_str().unwrap_or(""),
    );
    if !notes.is_empty() {
        c.push_str(&format!("\nShared notes:\n{notes}\n"));
    }
    if !diff.is_empty() {
        let mut end = diff.len().min(MAX_CONTEXT_DIFF);
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        c.push_str(&format!("\nShared git diff{}:\n{}\n", if end < diff.len() { " (truncated)" } else { "" }, &diff[..end]));
    }
    c.push_str(&format!(
        "\nTo answer: call the AgentMux MCP tool `mux_reply` with ticket \"{}\" and your answer as `text`. The asker only receives what you send through `mux_reply`.",
        ticket["id"].as_str().unwrap_or("")
    ));
    c
}
