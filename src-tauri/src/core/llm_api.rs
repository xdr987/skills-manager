use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Minimal OpenAI-compatible chat-completions client, good for Zhipu GLM,
/// Moonshot Kimi, DeepSeek and any other provider exposing the same shape.
///
/// Only what the notes summarizer needs: one system + one user message, a
/// small token budget, and the first choice's text back.

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
    error: Option<ChatErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: Option<ChatMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatErrorBody {
    message: Option<String>,
}

/// Normalize a user-supplied base URL into the full chat-completions endpoint.
/// Accepts the bare API root (`https://open.bigmodel.cn/api/paas/v4`) or a
/// URL that already ends in `/chat/completions`.
pub fn completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

/// Ask the model to compress `text` into a short Chinese phrase, guided by
/// `system_prompt`. Returns the trimmed content of the first choice.
///
/// LLM endpoints used here (Zhipu, Kimi, DeepSeek) are mainland-China
/// services: like the Baidu client this connects direct — through an
/// overseas proxy node the TLS handshake fails.
pub fn chat_completion(
    base_url: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_text: &str,
    max_tokens: u32,
) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("skills-manager")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;
    let endpoint = completions_endpoint(base_url);

    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_text }
        ],
        "max_tokens": max_tokens,
        "temperature": 0.3
    });

    let response: ChatCompletionResponse = client
        .post(&endpoint)
        .bearer_auth(api_key.trim())
        .json(&body)
        .send()
        .context("Failed to reach the LLM API")?
        .json()
        .context("Failed to parse the LLM API response")?;

    if let Some(err) = response.error {
        bail!("LLM API error: {}", err.message.unwrap_or_default());
    }

    let content = response
        .choices
        .first()
        .and_then(|choice| choice.message.as_ref())
        .and_then(|message| message.content.clone())
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty());

    match content {
        Some(text) => Ok(text),
        None => bail!("LLM API returned an empty answer"),
    }
}

#[cfg(test)]
mod tests {
    use super::completions_endpoint;

    #[test]
    fn endpoint_normalization() {
        assert_eq!(
            completions_endpoint("https://open.bigmodel.cn/api/paas/v4"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            completions_endpoint("https://api.deepseek.com/chat/completions/"),
            "https://api.deepseek.com/chat/completions"
        );
    }
}
