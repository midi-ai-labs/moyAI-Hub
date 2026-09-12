//! Local administrator initiated metadata discovery. Never exposed as a network API.
use std::collections::BTreeSet;
use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::Availability;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_MODELS: usize = 2048;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub label: String,
    pub availability: Availability,
    pub capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Discovery {
    pub endpoint: String,
    pub provider_profile: String,
    pub models: Vec<DiscoveredModel>,
}

#[derive(Debug)]
pub struct DiscoveryError {
    pub availability: Availability,
    pub message: String,
}
impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl From<String> for DiscoveryError {
    fn from(message: String) -> Self {
        Self {
            availability: Availability::Unknown,
            message,
        }
    }
}
impl From<&str> for DiscoveryError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

pub fn normalize_endpoint(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let input = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    let mut url =
        Url::parse(&input).map_err(|_| "IP:port または HTTP(S) URL を入力してください。")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("認証情報・query・fragment のない HTTP(S) URL を指定してください。".into());
    }
    let path = url
        .path()
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_owned();
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub async fn discover(raw: &str, profile: &str) -> Result<Discovery, DiscoveryError> {
    let endpoint = normalize_endpoint(raw)?;
    let path = crate::provider::ProviderProfile::parse(profile)
        .ok_or("この接続方式は利用できません。")?
        .metadata_path();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "モデル確認を開始できません。")?;
    let mut response = client
        .get(format!("{endpoint}/{path}"))
        .send()
        .await
        .map_err(|error| DiscoveryError {
            availability: if error.is_connect() {
                Availability::Offline
            } else {
                Availability::Unknown
            },
            message: "接続・受信を完了できません。ホストの起動状態・アドレスを確認してください。"
                .into(),
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "モデル一覧を取得できません（HTTP {}）。接続方式と認証要否を確認してください。",
            response.status().as_u16()
        )
        .into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("モデル一覧がサイズ上限を超えています。".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "モデル一覧の受信に失敗しました。")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err("モデル一覧がサイズ上限を超えています。".into());
        }
        body.extend_from_slice(&chunk);
    }
    let payload: Value =
        serde_json::from_slice(&body).map_err(|_| "モデル一覧の形式を確認できません。")?;
    let models = parse_models(&payload, profile)?;
    Ok(Discovery {
        endpoint,
        provider_profile: profile.to_owned(),
        models,
    })
}

pub fn parse_models(payload: &Value, profile: &str) -> Result<Vec<DiscoveredModel>, String> {
    let native = crate::provider::ProviderProfile::parse(profile)
        .ok_or("この接続方式は利用できません。")?
        .native_metadata();
    let entries = if native {
        payload.get("models").or(Some(payload))
    } else {
        payload.get("data")
    }
    .and_then(Value::as_array)
    .ok_or("モデル一覧の形式を確認できません。")?;
    if entries.len() > MAX_MODELS {
        return Err("モデル数が上限を超えています。".into());
    }
    let mut models = Vec::new();
    for entry in entries {
        if native
            && entry
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t != "llm")
        {
            continue;
        }
        let id = if native {
            entry.get("key").or_else(|| entry.get("id"))
        } else {
            entry.get("id")
        }
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
        if id.is_empty() || id.len() > 256 {
            return Err("モデル一覧の ID を確認できません。".into());
        }
        // A generic /v1/models response proves membership only, never readiness.
        let availability = if native {
            match entry
                .get("loaded_instances")
                .or_else(|| entry.get("loadedInstances"))
                .and_then(Value::as_array)
            {
                Some(instances) if instances.is_empty() => Availability::JitPending,
                Some(_) => Availability::Ready,
                None => Availability::Unknown,
            }
        } else {
            Availability::Unknown
        };
        let mut capabilities = BTreeSet::new();
        for (name, fields) in [
            ("tools", &["tools", "trained_for_tool_use"][..]),
            ("vision", &["vision", "images"][..]),
            ("reasoning", &["reasoning"][..]),
        ] {
            if fields.iter().any(|field| {
                entry
                    .get("capabilities")
                    .and_then(|v| v.get(field))
                    .and_then(Value::as_bool)
                    == Some(true)
            }) {
                capabilities.insert(name.to_owned());
            }
        }
        models.push(DiscoveredModel {
            id: id.to_owned(),
            label: entry
                .get("display_name")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(id)
                .chars()
                .take(160)
                .collect(),
            availability,
            capabilities,
        });
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn generic_listing_is_never_ready() {
        let rows = parse_models(
            &json!({"data":[{"id":"alpha","loaded_instances":[{}]}]}),
            "openai_compatible_chat",
        )
        .unwrap();
        assert_eq!(rows[0].availability, Availability::Unknown);
    }

    #[test]
    fn malformed_rows_do_not_become_evidence_of_missing_models() {
        assert!(parse_models(&json!({"data":[{}]}), "openai_compatible_chat").is_err());
        assert!(parse_models(&json!({"models":[{"type":"llm"}]}), "lmstudio_chat").is_err());
        assert!(
            parse_models(&json!({"data":[]}), "openai_compatible_chat")
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn native_presence_and_load_are_separate_facts() {
        let rows = parse_models(&json!({"models":[{"key":"a","type":"llm","loaded_instances":[]},{"key":"b","type":"llm","loaded_instances":[{}]},{"key":"c","type":"llm"},{"key":"embed","type":"embedding"}]}), "lmstudio_chat").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].availability, Availability::JitPending);
        assert_eq!(rows[1].availability, Availability::Ready);
        assert_eq!(rows[2].availability, Availability::Unknown);
    }
    #[test]
    fn generation_api_does_not_change_the_selected_metadata_dialect() {
        for profile in ["openai_compatible_chat", "openai_compatible_responses"] {
            let rows = parse_models(&json!({"data":[{"id":"generic"}]}), profile).unwrap();
            assert_eq!(rows[0].id, "generic");
            assert_eq!(rows[0].availability, Availability::Unknown);
            assert!(parse_models(&json!({"models":[{"key":"native"}]}), profile).is_err());
        }
        for profile in ["lmstudio_chat", "lmstudio_responses"] {
            let rows = parse_models(
                &json!({"models":[{"key":"native","type":"llm","loaded_instances":[]}]}),
                profile,
            )
            .unwrap();
            assert_eq!(rows[0].id, "native");
            assert_eq!(rows[0].availability, Availability::JitPending);
            assert!(parse_models(&json!({"data":[{"id":"generic"}]}), profile).is_err());
        }
        assert!(parse_models(&json!({"data":[]}), "unknown_profile").is_err());
    }
    #[test]
    fn endpoint_canonicalization_rejects_embedded_secrets() {
        assert_eq!(
            normalize_endpoint("127.0.0.1:1234/v1/").unwrap(),
            "http://127.0.0.1:1234"
        );
        for input in [
            "http://user:secret@localhost:1234",
            "http://localhost/?key=secret",
            "file:///etc/passwd",
        ] {
            assert!(normalize_endpoint(input).is_err());
        }
    }
}
