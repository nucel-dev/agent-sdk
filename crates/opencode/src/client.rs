//! OpenCode HTTP client.

use nucel_agent_core::{AgentCost, AgentError, AgentResponse, Result, SpawnConfig};
use serde_json::json;

/// HTTP client for OpenCode server.
pub struct OpencodeClient {
    http: reqwest::Client,
    base_url: String,
    #[allow(dead_code)]
    directory: Option<String>,
}

impl OpencodeClient {
    pub fn new(
        base_url: &str,
        _api_key: Option<&str>,
        directory: Option<&str>,
    ) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();

        if let Some(dir) = directory {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(dir) {
                headers.insert("x-opencode-directory", val);
            }
        }

        let mut builder = reqwest::Client::builder();
        if !headers.is_empty() {
            builder = builder.default_headers(headers);
        }

        Self {
            http: builder.build().expect("failed to build reqwest client"),
            base_url: base_url.to_string(),
            directory: directory.map(String::from),
        }
    }

    /// Create a new session on the OpenCode server.
    pub async fn create_session(&self) -> Result<serde_json::Value> {
        let resp = self
            .http
            .post(format!("{}/session", self.base_url))
            .json(&json!({}))
            .send()
            .await
            .map_err(|e| AgentError::Provider {
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

        // Add model if specified.
        if let Some(model) = &config.model {
            body["model"] = json!({ "modelID": model });
        }

        // Add system prompt if specified.
        if let Some(system) = &config.system_prompt {
            body["system"] = json!(system);
        }

        let resp = self
            .http
            .post(format!("{}/session/{}/prompt", self.base_url, session_id))
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::Provider {
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

        // Extract cost if available.
        let cost_usd = data
            .get("cost")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);

        if cost_usd > budget {
            return Err(AgentError::BudgetExceeded {
                limit: budget,
                spent: cost_usd,
            });
        }

        Ok(AgentResponse {
            content,
            cost: AgentCost {
                total_usd: cost_usd,
                ..Default::default()
            },
            confidence: None,
            requests_escalation: false,
            tool_calls: vec![],
        })
    }
}
