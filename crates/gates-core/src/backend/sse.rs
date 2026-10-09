//! The stream of an OpenAI-style chat completion (llama-server's
//! `/v1/chat/completions` with `"stream": true`): server-sent events, one
//! `data: {json}` line each, ending with `data: [DONE]`.

use serde_json::Value;

/// What one line of the stream says.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// A piece of the reply.
    Text(String),
    /// The server's tokens per second (llama-server's last chunk carries it).
    Speed(f64),
    /// A piece of a tool call: the call at `index` gets its id and name
    /// once, then its arguments a piece at a time.
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    /// The server reports an error in the stream.
    Error(String),
    /// The stream is over.
    Done,
}

/// Every event in one line: a chunk can carry text and its timings.
/// Blank lines, comments (`:`) and other fields give nothing.
pub fn parse(line: &str) -> Vec<Line> {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(data) = line.strip_prefix("data:") else {
        // llama-server reports an error before streaming as `error: {...}`.
        if let Some(error) = line.strip_prefix("error:") {
            return vec![Line::Error(error_message(error.trim()))];
        }
        return Vec::new();
    };
    let data = data.trim();
    if data == "[DONE]" {
        return vec![Line::Done];
    }
    let Ok(json) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    if let Some(error) = json.get("error") {
        return vec![Line::Error(error_text(error))];
    }
    let mut out = Vec::new();
    let content = json
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !content.is_empty() {
        out.push(Line::Text(content.to_string()));
    }
    if let Some(calls) = json
        .pointer("/choices/0/delta/tool_calls")
        .and_then(Value::as_array)
    {
        for call in calls {
            let text = |p: &str| call.pointer(p).and_then(Value::as_str).map(str::to_string);
            out.push(Line::ToolCall {
                index: call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                id: text("/id"),
                name: text("/function/name"),
                arguments: text("/function/arguments").unwrap_or_default(),
            });
        }
    }
    if let Some(speed) = json
        .pointer("/timings/predicted_per_second")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s > 0.0)
    {
        out.push(Line::Speed(speed));
    }
    out
}

/// The message of an error object (`{"message": ...}`), or its text.
pub fn error_text(error: &Value) -> String {
    let text = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| error.as_str().map(str::to_string))
        .unwrap_or_else(|| error.to_string());
    shorten(&text)
}

/// An error body as llama-server sends it, `{"error": {...}}`, or plain text.
pub fn error_message(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(json) => error_text(json.get("error").unwrap_or(&json)),
        Err(_) => shorten(body.trim()),
    }
}

/// At most 300 characters, on one line: it is shown in a banner.
fn shorten(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 300 {
        one_line
    } else {
        format!("{}…", one_line.chars().take(300).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_as_llama_server_sends_it() {
        let stream = [
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null}}],"object":"chat.completion.chunk"}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"content":"Hel"}}],"object":"chat.completion.chunk"}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"content":"lo"}}],"object":"chat.completion.chunk"}"#,
            "",
            r#"data: {"choices":[{"finish_reason":"stop","index":0,"delta":{}}],"timings":{"prompt_n":5,"predicted_n":2,"predicted_per_second":41.7}}"#,
            "",
            "data: [DONE]",
        ];
        let events: Vec<Line> = stream.iter().flat_map(|l| parse(l)).collect();
        assert_eq!(
            events,
            vec![
                Line::Text("Hel".into()),
                Line::Text("lo".into()),
                Line::Speed(41.7),
                Line::Done
            ]
        );
    }

    #[test]
    fn text_and_timings_in_one_chunk() {
        let e = parse(
            r#"data: {"choices":[{"delta":{"content":"!"}}],"timings":{"predicted_per_second":12.5}}"#,
        );
        assert_eq!(e, vec![Line::Text("!".into()), Line::Speed(12.5)]);
    }

    #[test]
    fn errors_in_and_before_the_stream() {
        assert_eq!(
            parse(
                r#"data: {"error":{"code":500,"message":"the context is full","type":"server_error"}}"#
            ),
            vec![Line::Error("the context is full".into())]
        );
        assert_eq!(
            parse(r#"error: {"code":400,"message":"bad request"}"#),
            vec![Line::Error("bad request".into())]
        );
    }

    #[test]
    fn tool_calls_come_in_pieces() {
        let first = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"read_file"}}]}}]}"#;
        let more = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}"#;
        assert_eq!(
            parse(first),
            vec![Line::ToolCall {
                index: 0,
                id: Some("c1".into()),
                name: Some("read_file".into()),
                arguments: String::new()
            }]
        );
        assert_eq!(
            parse(more),
            vec![Line::ToolCall {
                index: 0,
                id: None,
                name: None,
                arguments: "{\"path\":".into()
            }]
        );
    }

    #[test]
    fn noise_is_ignored() {
        assert!(parse(": keep-alive").is_empty());
        assert!(parse("event: message").is_empty());
        assert!(parse("data: not json").is_empty());
        assert!(parse(r#"data: {"timings":{"predicted_per_second":0}}"#).is_empty());
    }

    #[test]
    fn error_bodies() {
        assert_eq!(
            error_message(r#"{"error":{"message":"Invalid API Key"}}"#),
            "Invalid API Key"
        );
        assert_eq!(error_message("plain\ntext"), "plain text");
        assert!(error_message(&"x".repeat(1000)).chars().count() <= 301);
    }
}
