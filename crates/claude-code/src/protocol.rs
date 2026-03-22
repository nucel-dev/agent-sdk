//! JSONL protocol parsing for Claude Code CLI output.
//!
//! Supports two output modes:
//! - `--output-format json` — single JSON result object
//! - `--output-format stream-json` — streaming JSONL (system, assistant, result)

use nucel_agent_core::{AgentCost, AgentError, AgentResponse, Result};
use serde_json::Value;

/// Token usage from the CLI.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// Model-specific usage breakdown.
#[derive(Debug, Clone, Default)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
    pub model_id: String,
}

/// Parsed message from Claude Code CLI.
#[derive(Debug)]
pub enum ClaudeMessage {
    /// System init message (session_id, model, tools, MCP servers).
    SystemInit {
        session_id: String,
        model: String,
        tools: Vec<String>,
    },
    /// Assistant text response (streaming).
    Assistant {
        text: String,
        usage: Option<TokenUsage>,
        session_id: String,
    },
    /// Rate limit event.
    RateLimit { session_id: String },
    /// Final result with full cost breakdown.
    Result {
        text: String,
        is_error: bool,
        cost: AgentCost,
        session_id: String,
        duration_ms: u64,
        num_turns: u32,
    },
    /// Other message types (tool_use, thinking, etc.)
    Other,
}

/// Parse a single JSONL line into a Claude message.
pub fn parse_message(line: &str) -> Result<ClaudeMessage> {
    let v: Value = serde_json::from_str(line).map_err(|e| AgentError::Provider {
        provider: "claude-code".into(),
        message: format!("JSON parse error: {e}"),
    })?;

    let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
    let session_id = v
        .get("session_id")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    match msg_type {
        "system" => {
            let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
            if subtype == "init" {
                let model = v
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let tools = v
                    .get("tools")
                    .and_then(|t| t.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(ClaudeMessage::SystemInit {
                    session_id,
                    model,
                    tools,
                })
            } else {
                Ok(ClaudeMessage::Other)
            }
        }
        "assistant" => {
            let message = &v["message"];
            let content = &message["content"];

            // Extract text from content blocks.
            let mut text = String::new();
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(t);
                            }
                        }
                        "tool_use" => {
                            // Tool calls are part of the conversation but not text output.
                            // Log them for debugging but don't include in text.
                            let tool_name = block
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown");
                            tracing::debug!(tool = %tool_name, "tool_use in assistant message");
                        }
                        "thinking" => {
                            // Extended thinking content — don't include in text output.
                            tracing::debug!("thinking block in assistant message");
                        }
                        _ => {}
                    }
                }
            }

            // Extract usage.
            let usage = message.get("usage").map(|u| TokenUsage {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                cache_read_input_tokens: u
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_creation_input_tokens: u
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            });

            Ok(ClaudeMessage::Assistant {
                text,
                usage,
                session_id,
            })
        }
        "rate_limit_event" => Ok(ClaudeMessage::RateLimit { session_id }),
        "result" => {
            let result_text = v
                .get("result")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string();
            let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
            let total_cost_usd = v
                .get("total_cost_usd")
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);
            let duration_ms = v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0);
            let num_turns = v
                .get("num_turns")
                .and_then(|n| v.get("num_turns").and_then(|x| x.as_u64()))
                .unwrap_or(1) as u32;

            // Extract detailed usage from usage object.
            let usage = &v["usage"];
            let input_tokens = usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // Build cost from modelUsage if available (more accurate).
            let mut model_costs: Vec<ModelUsage> = Vec::new();
            if let Some(model_usage) = v.get("modelUsage").and_then(|m| m.as_object()) {
                for (model_id, data) in model_usage {
                    model_costs.push(ModelUsage {
                        input_tokens: data
                            .get("inputTokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        output_tokens: data
                            .get("outputTokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cache_read_input_tokens: data
                            .get("cacheReadInputTokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cache_creation_input_tokens: data
                            .get("cacheCreationInputTokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cost_usd: data.get("costUSD").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        model_id: model_id.clone(),
                    });
                }
            }

            Ok(ClaudeMessage::Result {
                text: result_text,
                is_error,
                cost: AgentCost {
                    input_tokens,
                    output_tokens,
                    total_usd: total_cost_usd,
                },
                session_id,
                duration_ms,
                num_turns,
            })
        }
        _ => Ok(ClaudeMessage::Other),
    }
}

/// Parse a complete non-streaming JSON result into an AgentResponse.
pub fn parse_single_result(json: &str) -> Result<AgentResponse> {
    let msg = parse_message(json)?;
    match msg {
        ClaudeMessage::Result {
            text,
            is_error,
            cost,
            ..
        } => {
            if is_error {
                return Err(AgentError::Provider {
                    provider: "claude-code".into(),
                    message: format!("agent returned error: {text}"),
                });
            }
            Ok(AgentResponse {
                content: text,
                cost,
                ..Default::default()
            })
        }
        _ => Err(AgentError::Provider {
            provider: "claude-code".into(),
            message: "expected result message, got something else".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_system_init() {
        let line = r#"{"type":"system","subtype":"init","cwd":"/tmp","session_id":"abc-123","tools":["Bash","Read","Edit"],"model":"claude-opus-4-6","permissionMode":"default"}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::SystemInit {
                session_id,
                model,
                tools,
            } => {
                assert_eq!(session_id, "abc-123");
                assert_eq!(model, "claude-opus-4-6");
                assert_eq!(tools.len(), 3);
                assert!(tools.contains(&"Bash".to_string()));
            }
            _ => panic!("expected SystemInit"),
        }
    }

    #[test]
    fn parse_assistant_text_block() {
        let line = r#"{"type":"assistant","message":{"model":"claude-opus-4-6","id":"msg_012","type":"message","role":"assistant","content":[{"type":"text","text":"Hello world"}],"stop_reason":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":5,"cache_read_input_tokens":100,"output_tokens":5}},"session_id":"sess-456"}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::Assistant {
                text,
                usage,
                session_id,
            } => {
                assert_eq!(text, "Hello world");
                assert_eq!(session_id, "sess-456");
                let u = usage.unwrap();
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 5);
                assert_eq!(u.cache_read_input_tokens, 100);
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn parse_assistant_with_tool_use() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls"}}]},"session_id":"s1"}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::Assistant { text, .. } => {
                assert!(text.is_empty(), "tool_use should not produce text");
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn parse_result_with_model_usage() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1953,"num_turns":1,"result":"Hello!","stop_reason":"end_turn","session_id":"s1","total_cost_usd":0.06237,"usage":{"input_tokens":3,"cache_creation_input_tokens":9426,"cache_read_input_tokens":6285,"output_tokens":12},"modelUsage":{"claude-opus-4-6[1m]":{"inputTokens":3,"outputTokens":12,"cacheReadInputTokens":6285,"cacheCreationInputTokens":9426,"costUSD":0.06237,"contextWindow":1000000}}}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::Result {
                text,
                is_error,
                cost,
                duration_ms,
                ..
            } => {
                assert_eq!(text, "Hello!");
                assert!(!is_error);
                assert!((cost.total_usd - 0.06237).abs() < 0.001);
                assert_eq!(cost.input_tokens, 3);
                assert_eq!(cost.output_tokens, 12);
                assert_eq!(duration_ms, 1953);
            }
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn parse_result_with_error() {
        let line = r#"{"type":"result","result":"Error occurred","total_cost_usd":0.01,"is_error":true,"session_id":"s1"}"#;
        let msg = parse_message(line).unwrap();
        match msg {
            ClaudeMessage::Result { is_error, .. } => assert!(is_error),
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn parse_rate_limit_event() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"},"session_id":"s1"}"#;
        let msg = parse_message(line).unwrap();
        assert!(matches!(msg, ClaudeMessage::RateLimit { .. }));
    }

    #[test]
    fn parse_unknown_type_returns_other() {
        let line = r#"{"type":"tool_result","content":"output","session_id":"s1"}"#;
        let msg = parse_message(line).unwrap();
        assert!(matches!(msg, ClaudeMessage::Other));
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_message("not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_single_result_success() {
        let json = r#"{"type":"result","result":"Done","total_cost_usd":0.05,"is_error":false,"session_id":"s1"}"#;
        let resp = parse_single_result(json).unwrap();
        assert_eq!(resp.content, "Done");
        assert!((resp.cost.total_usd - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_single_result_error() {
        let json = r#"{"type":"result","result":"Failed","total_cost_usd":0.01,"is_error":true,"session_id":"s1"}"#;
        let result = parse_single_result(json);
        assert!(result.is_err());
    }
}
