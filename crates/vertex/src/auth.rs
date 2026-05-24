//! Auth abstraction for Vertex.
//!
//! Real workloads use `gcp_auth` to mint an OAuth2 bearer token from
//! Application Default Credentials. Tests bypass GCP entirely by passing
//! a [`StaticToken`] provider.

use async_trait::async_trait;

use nucel_agent_core::{AgentError, Result};

/// Resolve a bearer token for the Vertex AI scope.
#[async_trait]
pub trait TokenProvider: Send + Sync {
    /// Returns the raw token string (no `"Bearer "` prefix).
    async fn token(&self) -> Result<String>;
}

/// Static token — useful for tests or pre-minted service-account tokens.
pub struct StaticToken {
    token: String,
}

impl StaticToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

#[async_trait]
impl TokenProvider for StaticToken {
    async fn token(&self) -> Result<String> {
        Ok(self.token.clone())
    }
}

/// GCP Application Default Credentials. Resolves via `gcp_auth` —
/// honors `GOOGLE_APPLICATION_CREDENTIALS`, gcloud user creds, GCE
/// metadata server, etc.
pub struct AdcToken {
    provider: std::sync::Arc<dyn gcp_auth::TokenProvider>,
}

impl AdcToken {
    /// Lazily resolve the ADC chain. Returns an error if no provider is
    /// available (e.g. no `gcloud auth application-default login` and no
    /// metadata server reachable).
    pub async fn discover() -> Result<Self> {
        let provider = gcp_auth::provider().await.map_err(|e| AgentError::Config(
            format!("Vertex: GCP credentials not found ({e}). Configure via \
                     GOOGLE_APPLICATION_CREDENTIALS or `gcloud auth \
                     application-default login`."),
        ))?;
        Ok(Self { provider })
    }
}

#[async_trait]
impl TokenProvider for AdcToken {
    async fn token(&self) -> Result<String> {
        let scopes = &["https://www.googleapis.com/auth/cloud-platform"];
        let token = self
            .provider
            .token(scopes)
            .await
            .map_err(|e| AgentError::Provider {
                provider: "vertex".into(),
                message: format!("failed to mint GCP token: {e}"),
            })?;
        Ok(token.as_str().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_token_returns_value() {
        let p = StaticToken::new("abc123");
        assert_eq!(p.token().await.unwrap(), "abc123");
    }
}
