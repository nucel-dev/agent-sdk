//! Best-effort price table for Claude models served on AWS Bedrock.
//!
//! Source: AWS Bedrock public pricing page (us-east-1, on-demand). These
//! numbers are USD per **million** input/output tokens. They are not
//! authoritative — the operator should treat reported cost as an
//! approximation and reconcile against their AWS invoice.
//!
//! When Bedrock launches a new region or tier and these numbers drift, the
//! reported `total_usd` is only ever an estimate — the authoritative token
//! counts (`input_tokens` / `output_tokens` / cache tokens) are always captured
//! on [`AgentCost`], so callers that need exact, region-specific pricing can
//! recompute cost themselves from those counts and ignore `total_usd`.

/// Per-million-token USD pricing.
#[derive(Debug, Clone, Copy)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl ModelPrice {
    /// Estimate USD cost for a given input/output token count.
    pub fn estimate(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        let input_cost = (input_tokens as f64) * self.input_per_mtok / 1_000_000.0;
        let output_cost = (output_tokens as f64) * self.output_per_mtok / 1_000_000.0;
        input_cost + output_cost
    }
}

/// Look up the on-demand price for a known Bedrock Claude model ID.
///
/// Returns `None` for unknown model IDs — caller should treat cost as 0
/// and log a warning. Match is by substring so cross-region inference
/// profile prefixes (e.g. `us.anthropic.claude-opus-4-7-...`) still hit.
pub fn lookup(model_id: &str) -> Option<ModelPrice> {
    // Claude Opus 4.7 (placeholder pricing — operator to verify)
    if model_id.contains("claude-opus-4-7") {
        return Some(ModelPrice {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        });
    }
    // Claude Opus 4 / 4.5
    if model_id.contains("claude-opus-4") {
        return Some(ModelPrice {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        });
    }
    // Claude Sonnet 4.x
    if model_id.contains("claude-sonnet-4") || model_id.contains("claude-3-5-sonnet") {
        return Some(ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        });
    }
    // Claude Haiku 4.x / 3.5
    if model_id.contains("claude-haiku") || model_id.contains("claude-3-5-haiku") {
        return Some(ModelPrice {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
        });
    }
    // Claude 3 Sonnet
    if model_id.contains("claude-3-sonnet") {
        return Some(ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        });
    }
    // Claude 3 Haiku
    if model_id.contains("claude-3-haiku") {
        return Some(ModelPrice {
            input_per_mtok: 0.25,
            output_per_mtok: 1.25,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_4_7_pricing_resolves() {
        let p = lookup("anthropic.claude-opus-4-7-20251024-v2:0").unwrap();
        assert!(p.input_per_mtok > 0.0);
        assert!(p.output_per_mtok > p.input_per_mtok);
    }

    #[test]
    fn cross_region_inference_profile_resolves() {
        let p = lookup("us.anthropic.claude-sonnet-4-20251015-v1:0").unwrap();
        assert!((p.input_per_mtok - 3.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(lookup("meta.llama3-70b-instruct-v1:0").is_none());
    }

    #[test]
    fn estimate_uses_per_million() {
        let p = ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        };
        // 1M in + 1M out = 18 USD
        let cost = p.estimate(1_000_000, 1_000_000);
        assert!((cost - 18.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_zero_tokens_is_zero() {
        let p = ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        };
        assert_eq!(p.estimate(0, 0), 0.0);
    }
}
