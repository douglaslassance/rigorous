//! Bone classification through a local Ollama model, using yello's prompts.

use crate::skeleton::format_hierarchy;
use crate::systems::{self, System};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

const SYSTEM_PROMPT: &str = include_str!("../prompts/classify_bones_system.md");
const USER_PROMPT: &str = include_str!("../prompts/classify_bones_user.md");
pub const DEFAULT_MODEL: &str = "qwen2.5:32b";
const DEFAULT_HOST: &str = "http://localhost:11434";
const TIMEOUT: Duration = Duration::from_secs(600);

pub fn classify(bones: &[(String, Option<String>)], model: &str) -> Result<Vec<System>> {
    let names: Vec<String> = bones.iter().map(|(name, _)| name.clone()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    let bone_list = sorted
        .iter()
        .map(|name| format!("  - {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    let user = USER_PROMPT
        .replace("{bone_list}", &bone_list)
        .replace("{hierarchy}", &format_hierarchy(bones));
    let host = host();
    let request = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user},
        ],
        "stream": false,
        "format": "json",
        "options": {"temperature": 0},
    });
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let response = match agent.post(&format!("{host}/api/chat")).send_json(request) {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => {
            bail!("Ollama does not have {model}. Pull it with `ollama pull {model}`.")
        }
        Err(ureq::Error::Status(code, response)) => bail!(
            "Ollama returned {code}: {}",
            response.into_string().unwrap_or_default()
        ),
        Err(error) => {
            bail!("cannot reach Ollama at {host} ({error}). Start it with `ollama serve`.")
        }
    };
    let body: Value = response
        .into_json()
        .context("Ollama returned an unreadable response")?;
    let content = body
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let data: Value = serde_json::from_str(content)
        .with_context(|| format!("Ollama returned invalid JSON: {content}"))?;
    let systems = systems::parse(&data, &names);
    if systems.is_empty() {
        bail!("could not classify any rig system from the model response: {content}");
    }
    Ok(systems)
}

/// Honor `OLLAMA_HOST`, which may omit the scheme and port.
fn host() -> String {
    let Ok(host) = std::env::var("OLLAMA_HOST") else {
        return DEFAULT_HOST.to_owned();
    };
    let host = host.trim().trim_end_matches('/');
    if host.is_empty() {
        return DEFAULT_HOST.to_owned();
    }
    let with_scheme = if host.contains("://") {
        host.to_owned()
    } else {
        format!("http://{host}")
    };
    let authority = with_scheme.split("://").nth(1).unwrap_or_default();
    if authority.contains(':') {
        with_scheme
    } else {
        format!("{with_scheme}:11434")
    }
}
