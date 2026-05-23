//! OpenCode HTTP client.

use nucel_agent_core::{AgentCost, AgentError, AgentResponse, Result, SpawnConfig};
use serde_json::json;

/// Default username for OpenCode HTTP basic auth when only a password (api_key)
/// is supplied. Matches upstream defaults.
const DEFAULT_BASIC_AUTH_USERNAME: &str = "opencode";

/// HTTP client for OpenCode server.
///
/// One `OpencodeClient` is meant to live for the duration of an executor +
/// session so the underlying `reqwest::Client` can pool HTTP connections.
#[derive(Clone)]
pub struct OpencodeClient {
    http: reqwest::Client,
    base_url: String,
    api_user: Option<String>,
    api_password: Option<String>,
    directory: Option<String>,
}

impl OpencodeClient {
    /// Build a new client.
    ///
    /// - `api_key`: HTTP basic-auth password (paired with
    ///   `OPENCODE_SERVER_USERNAME` env var or [`DEFAULT_BASIC_AUTH_USERNAME`]).
    ///   Falls back to `OPENCODE_SERVER_PASSWORD` env var when `None`.
    /// - `directory`: scopes server-side file ops; sent as `?directory=<path>`
    ///   query string (v2 contract) AND as the legacy
    ///   `x-opencode-directory` header for back-compat.
    pub fn new(base_url: &str, api_key: Option<&str>, directory: Option<&str>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();

        // Legacy directory header (back-compat with pre-v2 servers).
        if let Some(dir) = directory {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(dir) {
                headers.insert("x-opencode-directory", val);
            }
        }

        let mut builder = reqwest::Client::builder();
        if !headers.is_empty() {
            builder = builder.default_headers(headers);
        }

        // Resolve credentials:
        //   explicit api_key  → use as password
        //   else OPENCODE_SERVER_PASSWORD env var
        let password = api_key
            .map(String::from)
            .or_else(|| std::env::var("OPENCODE_SERVER_PASSWORD").ok());
        let username = std::env::var("OPENCODE_SERVER_USERNAME").ok();

        let (api_user, api_password) = match password {
            Some(pw) => (
                Some(username.unwrap_or_else(|| DEFAULT_BASIC_AUTH_USERNAME.to_string())),
                Some(pw),
            ),
            None => (None, None),
        };

        Self {
            http: builder.build().expect("failed to build reqwest client"),
            base_url: base_url.to_string(),
            api_user,
            api_password,
            directory: directory.map(String::from),
        }
    }

    /// Apply credentials and the optional `?directory=…` query.
    fn apply_common(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let (Some(user), Some(pw)) = (self.api_user.as_deref(), self.api_password.as_deref()) {
            req = req.basic_auth(user, Some(pw));
        }
        if let Some(dir) = &self.directory {
            req = req.query(&[("directory", dir.as_str())]);
        }
        req
    }

    /// Create a new session on the OpenCode server.
    pub async fn create_session(&self) -> Result<serde_json::Value> {
        let url = format!("{}/session", self.base_url);
        let req = self.apply_common(self.http.post(&url)).json(&json!({}));

        let resp = req.send().await.map_err(|e| AgentError::Provider {
            provider: "opencode".into(),
            message: format!("failed to create session: {e}"),
        })?;

        if !resp.status().is_success() {
            return Err(AgentError::Provider {
                provider: "opencode".into(),
                message: format!("session creation failed: {}", resp.status()),
            });
        }

        resp.json().await.map_err(|e| AgentError::Provider {
            provider: "opencode".into(),
            message: format!("failed to parse session response: {e}"),
        })
    }

    /// Send a prompt to a session.
    pub async fn prompt(
        &self,
        session_id: &str,
        prompt: &str,
        config: &SpawnConfig,
        budget: f64,
    ) -> Result<AgentResponse> {
        let mut body = json!({
            "parts": [
                {
                    "type": "text",
                    "text": prompt,
                }
            ],
        });

        // v2 model contract: { providerID, modelID } — split on "/".
        if let Some(model) = &config.model {
            body["model"] = build_model_body(model);
        }

        // Add system prompt if specified.
        if let Some(system) = &config.system_prompt {
            body["system"] = json!(system);
        }

        let url = format!("{}/session/{}/prompt", self.base_url, session_id);
        let req = self.apply_common(self.http.post(&url)).json(&body);

        let resp = req.send().await.map_err(|e| AgentError::Provider {
            provider: "opencode".into(),
            message: format!("prompt request failed: {e}"),
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(AgentError::Provider {
                provider: "opencode".into(),
                message: format!("prompt failed ({status}): {body_text}"),
            });
        }

        let data: serde_json::Value =
            resp.json().await.map_err(|e| AgentError::Provider {
                provider: "opencode".into(),
                message: format!("failed to parse prompt response: {e}"),
            })?;

        // Extract response text from parts.
        let mut content = String::new();
        if let Some(parts) = data.get("parts").and_then(|p| p.as_array()) {
            for part in parts {
                if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(text);
                    }
                }
            }
        }

        // If no parts, try the direct text field.
        if content.is_empty() {
            if let Some(text) = data.get("text").and_then(|t| t.as_str()) {
                content = text.to_string();
            }
        }

        // Extract cost.
        let cost_usd = data
            .get("cost")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);

        // Token usage — prefer the new `info.tokens` shape, fall back to the
        // legacy top-level `tokens` shape.
        let (input_tokens, output_tokens) = parse_tokens(&data);

        if cost_usd > budget {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: cost_usd,
            });
        }

        Ok(AgentResponse {
            content,
            cost: AgentCost {
                input_tokens,
                output_tokens,
                total_usd: cost_usd,
            },
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        })
    }

    /// Best-effort abort of an active session.
    pub async fn abort(&self, session_id: &str) -> Result<()> {
        let url = format!("{}/session/{}/abort", self.base_url, session_id);
        let req = self.apply_common(self.http.post(&url));
        match req.send().await {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::debug!(error = %e, session_id = %session_id, "opencode abort failed (best-effort)");
                Ok(())
            }
        }
    }
}

/// Split `provider/model` into `{ "providerID": …, "modelID": … }`.
/// If no `/`, omit `providerID` and let the server pick a default provider.
pub(crate) fn build_model_body(model: &str) -> serde_json::Value {
    match model.split_once('/') {
        Some((provider, model_id)) if !provider.is_empty() && !model_id.is_empty() => {
            json!({ "providerID": provider, "modelID": model_id })
        }
        _ => json!({ "modelID": model }),
    }
}

/// Parse tokens from either `info.tokens.{input,output}` (v2) or the
/// top-level `tokens.{input,output}` (legacy) shape.
fn parse_tokens(data: &serde_json::Value) -> (u64, u64) {
    let tokens = data
        .get("info")
        .and_then(|i| i.get("tokens"))
        .or_else(|| data.get("tokens"));

    match tokens {
        Some(t) => {
            let input = t
                .get("input")
                .or_else(|| t.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = t
                .get("output")
                .or_else(|| t.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            (input, output)
        }
        None => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_model_body_with_provider() {
        let b = build_model_body("anthropic/claude-sonnet-4");
        assert_eq!(b["providerID"], "anthropic");
        assert_eq!(b["modelID"], "claude-sonnet-4");
    }

    #[test]
    fn build_model_body_without_provider() {
        let b = build_model_body("claude-sonnet-4");
        assert_eq!(b["modelID"], "claude-sonnet-4");
        assert!(
            b.get("providerID").is_none(),
            "providerID must be omitted when model has no '/': {b:?}"
        );
    }

    #[test]
    fn build_model_body_empty_provider_segment_is_treated_as_no_provider() {
        // "/claude-sonnet-4" → no provider → no providerID.
        let b = build_model_body("/claude-sonnet-4");
        assert!(b.get("providerID").is_none(), "{b:?}");
    }

    #[test]
    fn parse_tokens_v2_info_shape() {
        let data = json!({
            "info": { "tokens": { "input": 12, "output": 34 } }
        });
        let (i, o) = parse_tokens(&data);
        assert_eq!(i, 12);
        assert_eq!(o, 34);
    }

    #[test]
    fn parse_tokens_legacy_top_level_shape() {
        let data = json!({
            "tokens": { "input": 7, "output": 9 }
        });
        let (i, o) = parse_tokens(&data);
        assert_eq!(i, 7);
        assert_eq!(o, 9);
    }

    #[test]
    fn parse_tokens_legacy_underscored_keys() {
        let data = json!({
            "tokens": { "input_tokens": 1, "output_tokens": 2 }
        });
        let (i, o) = parse_tokens(&data);
        assert_eq!(i, 1);
        assert_eq!(o, 2);
    }

    #[test]
    fn parse_tokens_missing_returns_zero() {
        let data = json!({});
        assert_eq!(parse_tokens(&data), (0, 0));
    }

    #[test]
    fn client_constructs_with_api_key_password() {
        let c = OpencodeClient::new("http://example.com", Some("secret"), None);
        assert_eq!(c.api_password.as_deref(), Some("secret"));
        // Default username when only password is provided.
        assert_eq!(c.api_user.as_deref(), Some("opencode"));
    }

    #[test]
    fn client_constructs_without_credentials() {
        // We don't want env from the test runner leaking into this; just
        // assert that passing None doesn't blow up.
        let c = OpencodeClient::new("http://example.com", None, None);
        let _ = c.base_url;
    }
}
