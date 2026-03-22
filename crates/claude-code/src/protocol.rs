//! JSONL protocol parsing for Claude Code CLI output.

use nucel_agent_core::Result;

/// Token usage information.
#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Parsed message from Claude Code CLI.
#[derive(Debug)]
pub enum ClaudeMessage {
    /// Assistant text response.
    Assistant {
        text: String,
        cost: Option<f64>,
        usage: Option<TokenUsage>,
    },
    /// Final result with session summary.
    Result {
        text: Option<String>,
        cost: Option<f64>,
        is_error: bool,
    },
    /// Other message types (system, stream events, etc.)
    Other,
}

/// Parse a single JSONL line into a Claude message.
pub fn parse_message(line: &str) -> Result<ClaudeMessage> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| nucel_agent_core::AgentError::Provider {
            provider: "claude-code".into(),
            message: format!("JSON parse error: {e}"),
        })?;

    let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");

    match msg_type {
        "assistant" => {
            let message = &v["message"];
            let content = &message["content"];

            let mut text = String::new();
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                    }
                }
            }

            let usage = message.get("usage").map(|u| TokenUsage {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            });

            Ok(ClaudeMessage::Assistant {
                text,
                cost: None, // Cost calculated from usage + pricing.
                usage,
            })
        }
        "result" => {
            let text = v.get("result").and_then(|r| r.as_str()).map(String::from);
            let cost = v.get("total_cost_usd").and_then(|c| c.as_f64());
            let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);

            Ok(ClaudeMessage::Result {
                text,
                cost,
                is_error,
            })
        }
        _ => Ok(ClaudeMessage::Other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_assistant_text_block() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello world"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::Assistant { text, usage, .. } => {
                assert_eq!(text, "Hello world");
                let u = usage.unwrap();
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 5);
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn parse_result_with_cost() {
        let line = r#"{"type":"result","result":"Done","total_cost_usd":0.05,"is_error":false}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::Result {
                text,
                cost,
                is_error,
            } => {
                assert_eq!(text, Some("Done".to_string()));
                assert_eq!(cost, Some(0.05));
                assert!(!is_error);
            }
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn parse_result_with_error() {
        let line = r#"{"type":"result","result":"Failed","total_cost_usd":0.01,"is_error":true}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::Result { is_error, .. } => assert!(is_error),
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn parse_unknown_type_returns_other() {
        let line = r#"{"type":"system","message":"started"}"#;
        let msg = parse_message(line).unwrap();
        assert!(matches!(msg, ClaudeMessage::Other));
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_message("not json");
        assert!(result.is_err());
    }
}
