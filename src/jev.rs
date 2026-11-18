use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, env, time::Duration};
use thiserror::Error;

pub const ENDPOINT: &str = "https://opencode.ai/inference/systemone/v1/systemone";
const MOODS: [&str; 6] = [
    "calm",
    "dark",
    "energetic",
    "melancholy",
    "quirky",
    "romantic",
];

pub const INTENT_CATEGORIES: [&str; 3] =
    ["search_knowledge_base", "talk_to_agents", "casual_greeting"];

#[derive(Clone, Debug, Deserialize)]
pub struct JevConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
}

fn default_model() -> String {
    "jev-1.13".into()
}
fn default_api_key_env() -> String {
    "OPENCODE_CONSOLE_API_KEY".into()
}
fn default_timeout_secs() -> u64 {
    60
}
fn default_endpoint() -> String {
    ENDPOINT.into()
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            api_key_env: default_api_key_env(),
            timeout_secs: default_timeout_secs(),
            endpoint: default_endpoint(),
        }
    }
}

#[derive(Debug, Error)]
pub enum JevError {
    #[error("JEV API key is not configured")]
    MissingApiKey,
    #[error("JEV request failed")]
    Request(#[from] reqwest::Error),
    #[error("JEV returned unsuccessful status {0}")]
    UpstreamStatus(StatusCode),
    #[error("JEV response is invalid: {0}")]
    InvalidResponse(&'static str),
}

#[derive(Debug, Serialize)]
pub struct SystemOneRequest<'a> {
    pub model: &'a str,
    pub state: &'a str,
    pub questions: BTreeMap<&'static str, ChoiceQuestion>,
}

#[derive(Debug, Serialize)]
pub struct ChoiceQuestion {
    #[serde(rename = "type")]
    pub question_type: &'static str,
    pub instructions: &'static str,
    pub criteria: BTreeMap<&'static str, &'static str>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SystemOneResponse {
    pub answers: BTreeMap<String, SystemOneAnswer>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SystemOneAnswer {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub answer_type: Option<String>,
    pub choice: Option<String>,
    pub confidence: Option<f64>,
    #[serde(default)]
    pub probabilities: BTreeMap<String, f64>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct MoodResponse {
    pub moods: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntentResponse {
    pub intent: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatCompletionRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatCompletionChoice {
    pub index: u32,
    pub message: ChatCompletionResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatCompletionResponseMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatCompletionChunkChoice {
    pub index: u32,
    pub delta: ChatCompletionChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ChatCompletionChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatErrorEnvelope {
    pub error: ChatErrorDetail,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Clone)]
pub struct JevClient {
    client: Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl JevClient {
    pub fn from_config(config: &JevConfig) -> Result<Self, JevError> {
        let api_key = env::var(&config.api_key_env)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .ok_or(JevError::MissingApiKey)?;
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;
        Ok(Self {
            client,
            endpoint: config.endpoint.clone(),
            model: config.model.clone(),
            api_key,
        })
    }

    #[cfg(test)]
    pub fn new_for_test(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.into(),
            model: model.into(),
            api_key: api_key.into(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn classify_mood(&self, text: &str) -> Result<MoodResponse, JevError> {
        let request = mood_request(&self.model, text);
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(JevError::UpstreamStatus(response.status()));
        }
        let parsed: SystemOneResponse = response
            .json()
            .await
            .map_err(|_| JevError::InvalidResponse("malformed response body from JEV upstream"))?;
        normalize_mood_answer(
            parsed
                .answers
                .get("mood")
                .ok_or(JevError::InvalidResponse("missing mood answer"))?,
        )
    }

    pub async fn classify_intent(
        &self,
        text: &str,
        model_override: Option<&str>,
    ) -> Result<IntentResponse, JevError> {
        let model = model_override
            .filter(|m| !m.trim().is_empty())
            .unwrap_or(&self.model);
        let request = intent_request(model, text);
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(JevError::UpstreamStatus(response.status()));
        }
        let parsed: SystemOneResponse = response
            .json()
            .await
            .map_err(|_| JevError::InvalidResponse("malformed response body from JEV upstream"))?;
        normalize_intent_answer(
            parsed
                .answers
                .get("intent")
                .ok_or(JevError::InvalidResponse("missing intent answer"))?,
        )
    }
}

impl JevClient {
    pub fn from_env_if_configured(config: &JevConfig) -> Result<Option<Self>, JevError> {
        match env::var(&config.api_key_env) {
            Ok(key) if !key.trim().is_empty() => Self::from_config(config).map(Some),
            _ => Ok(None),
        }
    }
}

pub fn mood_request<'a>(model: &'a str, text: &'a str) -> SystemOneRequest<'a> {
    let criteria = [
        ("calm", "Peaceful, serene, relaxed, gentle"),
        ("dark", "Grim, ominous, sinister, intense, heavy"),
        ("energetic", "High-energy, fast, vibrant, upbeat"),
        ("melancholy", "Sad, sorrowful, wistful, yearning"),
        ("quirky", "Playful, eccentric, unconventional, whimsical"),
        ("romantic", "Intimate, affectionate, loving, sentimental"),
    ]
    .into_iter()
    .collect();
    SystemOneRequest {
        model,
        state: text,
        questions: [(
            "mood",
            ChoiceQuestion {
                question_type: "choice",
                instructions: "What is the primary mood conveyed by the text?",
                criteria,
            },
        )]
        .into_iter()
        .collect(),
    }
}

pub fn normalize_mood_answer(answer: &SystemOneAnswer) -> Result<MoodResponse, JevError> {
    let mut moods = BTreeMap::new();
    for mood in MOODS {
        let value = *answer
            .probabilities
            .get(mood)
            .ok_or(JevError::InvalidResponse("missing mood probability"))?;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(JevError::InvalidResponse(
                "probability is not a finite value in [0, 1]",
            ));
        }
        moods.insert(mood.to_owned(), (value * 10_000.0).round() / 10_000.0);
    }
    Ok(MoodResponse { moods })
}

pub fn intent_request<'a>(model: &'a str, state: &'a str) -> SystemOneRequest<'a> {
    let criteria = [
        (
            "search_knowledge_base",
            "Search documentation, architecture, code, facts, or technical information in the knowledge base",
        ),
        (
            "talk_to_agents",
            "Talk to, direct, delegate tasks to, or collaborate with AI employees or agents",
        ),
        (
            "casual_greeting",
            "Casual greetings, pleasantries, small talk, or polite conversation",
        ),
    ]
    .into_iter()
    .collect();

    SystemOneRequest {
        model,
        state,
        questions: [(
            "intent",
            ChoiceQuestion {
                question_type: "choice",
                instructions: "Classify the user intent into exactly one category: search_knowledge_base, talk_to_agents, or casual_greeting.",
                criteria,
            },
        )]
        .into_iter()
        .collect(),
    }
}

pub fn normalize_intent_answer(answer: &SystemOneAnswer) -> Result<IntentResponse, JevError> {
    let choice = answer
        .choice
        .as_deref()
        .ok_or(JevError::InvalidResponse("missing intent choice"))?;

    if !INTENT_CATEGORIES.contains(&choice) {
        return Err(JevError::InvalidResponse(
            "unexpected intent category choice",
        ));
    }

    let confidence = answer
        .confidence
        .ok_or(JevError::InvalidResponse("missing intent confidence"))?;

    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(JevError::InvalidResponse(
            "confidence is not a finite value in [0, 1]",
        ));
    }

    let mut probabilities = BTreeMap::new();
    for category in INTENT_CATEGORIES {
        let value = *answer
            .probabilities
            .get(category)
            .ok_or(JevError::InvalidResponse(
                "missing intent category probability",
            ))?;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(JevError::InvalidResponse(
                "probability is not a finite value in [0, 1]",
            ));
        }
        probabilities.insert(category.to_owned(), (value * 10_000.0).round() / 10_000.0);
    }

    Ok(IntentResponse {
        intent: choice.to_owned(),
        confidence: (confidence * 10_000.0).round() / 10_000.0,
        probabilities,
    })
}

pub fn extract_intent_prompt(messages: &[ChatMessage]) -> Result<String, &'static str> {
    if messages.is_empty() {
        return Err("messages must not be empty");
    }

    let last_user_idx = messages
        .iter()
        .rposition(|m| {
            m.role.eq_ignore_ascii_case("user")
                && m.content.as_ref().is_some_and(|c| !c.trim().is_empty())
        })
        .ok_or("at least one non-empty user message is required")?;

    let latest_user_msg = messages[last_user_idx].content.as_ref().unwrap().trim();

    let preceding: Vec<_> = messages[..last_user_idx]
        .iter()
        .filter_map(|m| {
            let text = m.content.as_deref().unwrap_or("").trim();
            if text.is_empty() {
                None
            } else {
                Some(format!("{}: {}", m.role, text))
            }
        })
        .collect();

    if preceding.is_empty() {
        Ok(latest_user_msg.to_string())
    } else {
        Ok(format!(
            "Context:\n{}\n\nUser: {}",
            preceding.join("\n"),
            latest_user_msg
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_system_one_request_and_all_mood_criteria() {
        let value = serde_json::to_value(mood_request("jev-1.13", "example")).unwrap();
        assert_eq!(value["model"], "jev-1.13");
        assert_eq!(value["state"], "example");
        assert_eq!(value["questions"]["mood"]["type"], "choice");
        assert_eq!(
            value["questions"]["mood"]["criteria"]
                .as_object()
                .unwrap()
                .len(),
            6
        );
    }

    #[test]
    fn parses_and_normalizes_probabilities() {
        let response: SystemOneResponse = serde_json::from_str(r#"{"answers":{"mood":{"type":"choice","choice":"calm","confidence":0.9,"probabilities":{"calm":0.123456,"dark":0.2,"energetic":0.1,"melancholy":0.2,"quirky":0.2,"romantic":0.176544}}}}"#).unwrap();
        let result = normalize_mood_answer(&response.answers["mood"]).unwrap();
        assert_eq!(result.moods.len(), 6);
        assert_eq!(result.moods["calm"], 0.1235);
    }

    #[test]
    fn rejects_missing_or_invalid_probabilities() {
        let missing = SystemOneAnswer {
            answer_type: None,
            choice: None,
            confidence: None,
            probabilities: BTreeMap::new(),
        };
        assert!(matches!(
            normalize_mood_answer(&missing),
            Err(JevError::InvalidResponse(_))
        ));
        let bad = SystemOneAnswer {
            probabilities: [("calm".to_owned(), f64::NAN)].into_iter().collect(),
            ..missing
        };
        assert!(matches!(
            normalize_mood_answer(&bad),
            Err(JevError::InvalidResponse(_))
        ));
    }

    #[test]
    fn serializes_system_one_intent_request_and_all_criteria() {
        let value = serde_json::to_value(intent_request("jev-1.13", "find documentation")).unwrap();
        assert_eq!(value["model"], "jev-1.13");
        assert_eq!(value["state"], "find documentation");
        assert_eq!(value["questions"]["intent"]["type"], "choice");
        let criteria = value["questions"]["intent"]["criteria"]
            .as_object()
            .unwrap();
        assert_eq!(criteria.len(), 3);
        assert!(criteria.contains_key("search_knowledge_base"));
        assert!(criteria.contains_key("talk_to_agents"));
        assert!(criteria.contains_key("casual_greeting"));
    }

    #[test]
    fn parses_and_normalizes_intent_answer() {
        let response: SystemOneResponse = serde_json::from_str(
            r#"{
                "answers": {
                    "intent": {
                        "type": "choice",
                        "choice": "search_knowledge_base",
                        "confidence": 0.987654,
                        "probabilities": {
                            "search_knowledge_base": 0.987654,
                            "talk_to_agents": 0.010000,
                            "casual_greeting": 0.002346
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let result = normalize_intent_answer(&response.answers["intent"]).unwrap();
        assert_eq!(result.intent, "search_knowledge_base");
        assert_eq!(result.confidence, 0.9877);
        assert_eq!(result.probabilities.len(), 3);
        assert_eq!(result.probabilities["search_knowledge_base"], 0.9877);
        assert_eq!(result.probabilities["talk_to_agents"], 0.01);
        assert_eq!(result.probabilities["casual_greeting"], 0.0023);
    }

    #[test]
    fn rejects_intent_answer_with_invalid_or_missing_fields() {
        let base_probs = [
            ("search_knowledge_base".to_string(), 0.9),
            ("talk_to_agents".to_string(), 0.05),
            ("casual_greeting".to_string(), 0.05),
        ]
        .into_iter()
        .collect();

        let missing_choice = SystemOneAnswer {
            answer_type: Some("choice".into()),
            choice: None,
            confidence: Some(0.9),
            probabilities: base_probs,
        };
        assert!(matches!(
            normalize_intent_answer(&missing_choice),
            Err(JevError::InvalidResponse(_))
        ));

        let unexpected_choice = SystemOneAnswer {
            choice: Some("unknown_intent".into()),
            ..missing_choice.clone()
        };
        assert!(matches!(
            normalize_intent_answer(&unexpected_choice),
            Err(JevError::InvalidResponse(_))
        ));

        let missing_conf = SystemOneAnswer {
            choice: Some("talk_to_agents".into()),
            confidence: None,
            ..missing_choice.clone()
        };
        assert!(matches!(
            normalize_intent_answer(&missing_conf),
            Err(JevError::InvalidResponse(_))
        ));

        let nan_conf = SystemOneAnswer {
            choice: Some("talk_to_agents".into()),
            confidence: Some(f64::NAN),
            ..missing_choice.clone()
        };
        assert!(matches!(
            normalize_intent_answer(&nan_conf),
            Err(JevError::InvalidResponse(_))
        ));

        let oob_conf = SystemOneAnswer {
            choice: Some("talk_to_agents".into()),
            confidence: Some(1.5),
            ..missing_choice.clone()
        };
        assert!(matches!(
            normalize_intent_answer(&oob_conf),
            Err(JevError::InvalidResponse(_))
        ));

        let missing_category = SystemOneAnswer {
            choice: Some("casual_greeting".into()),
            confidence: Some(0.95),
            probabilities: [("casual_greeting".to_string(), 0.95)]
                .into_iter()
                .collect(),
            ..missing_choice.clone()
        };
        assert!(matches!(
            normalize_intent_answer(&missing_category),
            Err(JevError::InvalidResponse(_))
        ));

        let nan_prob = SystemOneAnswer {
            choice: Some("casual_greeting".into()),
            confidence: Some(0.95),
            probabilities: [
                ("search_knowledge_base".to_string(), f64::NAN),
                ("talk_to_agents".to_string(), 0.05),
                ("casual_greeting".to_string(), 0.95),
            ]
            .into_iter()
            .collect(),
            ..missing_choice
        };
        assert!(matches!(
            normalize_intent_answer(&nan_prob),
            Err(JevError::InvalidResponse(_))
        ));
    }

    #[test]
    fn extracts_intent_prompt_from_single_user_message() {
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: Some("find docs".into()),
        }];
        assert_eq!(extract_intent_prompt(&messages).unwrap(), "find docs");
    }

    #[test]
    fn extracts_intent_prompt_with_preceding_context() {
        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: Some("You are an assistant.".into()),
            },
            ChatMessage {
                role: "user".into(),
                content: Some("Hello".into()),
            },
            ChatMessage {
                role: "assistant".into(),
                content: Some("Hi! How can I help?".into()),
            },
            ChatMessage {
                role: "user".into(),
                content: Some("What is the architecture?".into()),
            },
        ];
        let prompt = extract_intent_prompt(&messages).unwrap();
        assert!(prompt.contains(
            "Context:\nsystem: You are an assistant.\nuser: Hello\nassistant: Hi! How can I help?"
        ));
        assert!(prompt.ends_with("\n\nUser: What is the architecture?"));
    }

    #[test]
    fn extracts_latest_non_empty_user_message_ignoring_trailing_empty() {
        let messages = vec![
            ChatMessage {
                role: "assistant".into(),
                content: Some("answering".into()),
            },
            ChatMessage {
                role: "user".into(),
                content: Some("Latest real question".into()),
            },
            ChatMessage {
                role: "user".into(),
                content: Some("   ".into()),
            },
        ];
        assert_eq!(
            extract_intent_prompt(&messages).unwrap(),
            "Context:\nassistant: answering\n\nUser: Latest real question"
        );
    }

    #[test]
    fn extract_intent_prompt_rejects_missing_user_message_or_empty() {
        assert_eq!(
            extract_intent_prompt(&[]).unwrap_err(),
            "messages must not be empty"
        );

        let system_only = vec![ChatMessage {
            role: "system".into(),
            content: Some("system instructions".into()),
        }];
        assert_eq!(
            extract_intent_prompt(&system_only).unwrap_err(),
            "at least one non-empty user message is required"
        );

        let blank_user = vec![ChatMessage {
            role: "user".into(),
            content: Some("   ".into()),
        }];
        assert_eq!(
            extract_intent_prompt(&blank_user).unwrap_err(),
            "at least one non-empty user message is required"
        );
    }

    #[test]
    fn serializes_chat_completion_response_and_chunks() {
        let completion = ChatCompletionResponse {
            id: "chatcmpl-test".into(),
            object: "chat.completion".into(),
            created: 1234567890,
            model: "jev-1.13".into(),
            choices: vec![ChatCompletionChoice {
                index: 0,
                message: ChatCompletionResponseMessage {
                    role: "assistant".into(),
                    content: "{\"intent\":\"talk_to_agents\"}".into(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(ChatUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            }),
        };
        let json_val = serde_json::to_value(&completion).unwrap();
        assert_eq!(json_val["id"], "chatcmpl-test");
        assert_eq!(json_val["object"], "chat.completion");
        assert_eq!(json_val["choices"][0]["message"]["role"], "assistant");
        assert_eq!(json_val["choices"][0]["finish_reason"], "stop");

        let chunk = ChatCompletionChunk {
            id: "chatcmpl-test".into(),
            object: "chat.completion.chunk".into(),
            created: 1234567890,
            model: "jev-1.13".into(),
            choices: vec![ChatCompletionChunkChoice {
                index: 0,
                delta: ChatCompletionChunkDelta {
                    role: Some("assistant".into()),
                    content: None,
                },
                finish_reason: None,
            }],
        };
        let chunk_val = serde_json::to_value(&chunk).unwrap();
        assert_eq!(chunk_val["object"], "chat.completion.chunk");
        assert_eq!(chunk_val["choices"][0]["delta"]["role"], "assistant");
        assert!(chunk_val["choices"][0]["delta"].get("content").is_none());
        assert!(chunk_val["choices"][0]["finish_reason"].is_null());
    }
}
