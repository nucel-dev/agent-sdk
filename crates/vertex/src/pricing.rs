//! Per-million-token USD pricing for Claude on Vertex AI.
//!
//! Source: Anthropic's Vertex documentation (Vertex matches direct
//! Anthropic API list price 1:1 for Claude SKUs). These are best-effort —
//! operators should reconcile against their GCP invoice.

#[derive(Debug, Clone, Copy)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl ModelPrice {
    pub fn estimate(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        let input_cost = (input_tokens as f64) * self.input_per_mtok / 1_000_000.0;
        let output_cost = (output_tokens as f64) * self.output_per_mtok / 1_000_000.0;
        input_cost + output_cost
    }
}

/// Look up by Vertex model name (e.g. `claude-opus-4-7@20251024`,
/// `claude-sonnet-4@20251015`, `claude-haiku-4@20251015`).
pub fn lookup(model: &str) -> Option<ModelPrice> {
    if model.contains("claude-opus-4-7") || model.contains("claude-opus-4") {
        return Some(ModelPrice {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        });
    }
    if model.contains("claude-sonnet-4") || model.contains("claude-3-5-sonnet") {
        return Some(ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        });
    }
    if model.contains("claude-haiku-4") || model.contains("claude-3-5-haiku") {
        return Some(ModelPrice {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
        });
    }
    if model.contains("claude-3-sonnet") {
        return Some(ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        });
    }
    if model.contains("claude-3-haiku") {
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
    fn opus_resolves() {
        let p = lookup("claude-opus-4-7@20251024").unwrap();
        assert!((p.input_per_mtok - 15.0).abs() < 1e-9);
    }

    #[test]
    fn sonnet_resolves() {
        let p = lookup("claude-sonnet-4@20251015").unwrap();
        assert!((p.output_per_mtok - 15.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_returns_none() {
        assert!(lookup("gemini-1.5-pro").is_none());
    }

    #[test]
    fn estimate_arithmetic() {
        let p = ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        };
        assert!((p.estimate(1_000_000, 1_000_000) - 18.0).abs() < 1e-9);
    }
}
