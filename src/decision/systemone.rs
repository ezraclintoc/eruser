//! The System One client: Jev's own wire shape, over plain HTTP.
//!
//! The request is the API's, not ours:
//!
//! ```text
//! POST <endpoint>
//! {"state": "<what the model is looking at>",
//!  "model": "jev-latest",
//!  "questions": {"decision": {"type": "choice",
//!                             "instructions": "...",
//!                             "criteria": {"missing_info": "when they asked for details"}}}}
//! ```
//!
//! and the answer names the label it picked:
//!
//! ```text
//! 200 {"model": "jev-1.13.0",
//!      "answers": {"decision": {"type": "choice", "choice": "missing_info",
//!                               "probabilities": {"missing_info": 0.81},
//!                               "confidence": 0.82}}}
//! ```
//!
//! Everything the API returns is passed over rather than re-derived, except
//! for the one check that matters: a choice that was not on the list is
//! refused. The caller's options are the contract, not the model's opinion
//! of them.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Choice, Decider, Decision, Error};

/// The name every question is asked under. One question is asked at a time,
/// so the name is a constant rather than part of the API here.
const QUESTION: &str = "decision";

/// The only question type eruser asks. `score` and `noul` (a yes/no) exist in
/// the API and are not used yet.
const CHOICE: &str = "choice";

/// How many times a rate-limited request is tried again.
///
/// The API documents `429` and `529` and asks clients to back off rather than
/// retry immediately. Two retries is enough to ride out a burst without
/// holding a scanning run open: past that, the rules decide, which is where
/// the request would have landed with no model at all.
const RETRIES: u32 = 2;

/// How long to wait before the first retry. The second waits twice as long.
const RETRY_DELAY: Duration = Duration::from_millis(600);

/// Whether a status is one the API asks to be retried rather than reported.
fn retryable(status: u16) -> bool {
    matches!(status, 429 | 529)
}

/// A System One model behind an HTTP endpoint.
pub struct SystemOne {
    url: String,
    model: String,
    api_key: Option<String>,
    retry_delay: Duration,
    client: reqwest::Client,
}

/// What eruser sends.
#[derive(Serialize)]
struct Request<'a> {
    state: &'a str,
    model: &'a str,
    questions: BTreeMap<&'static str, Question<'a>>,
}

#[derive(Serialize)]
struct Question<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    instructions: &'a str,
    criteria: BTreeMap<&'a str, &'a str>,
}

/// What comes back.
#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    model: String,
    #[serde(default)]
    answers: BTreeMap<String, Answer>,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    choice: String,
    /// Absent when the service does not report one. An explicit `0.0` and a
    /// missing field mean different things, so this is not a plain `f32`.
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    probabilities: BTreeMap<String, f32>,
}

impl SystemOne {
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
        timeout: Duration,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();

        Self {
            url: resolve_endpoint(&endpoint.into()),
            model: model.into(),
            api_key,
            retry_delay: RETRY_DELAY,
            client,
        }
    }

    /// How long to wait before the first retry.
    ///
    /// Exposed so a test can retry without waiting; nothing else has a reason
    /// to change it.
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// The resolved URL, for logs and for the setup wizard to confirm.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// One request, however it turns out.
    async fn attempt(&self, request: &Request<'_>) -> Result<(u16, String), Error> {
        let mut send = self.client.post(&self.url).json(request);
        if let Some(key) = &self.api_key {
            send = send.bearer_auth(key);
        }

        let answer = send
            .send()
            .await
            .map_err(|error| Error::Unreachable(error.to_string()))?;

        let status = answer.status().as_u16();
        let body = answer
            .text()
            .await
            .map_err(|error| Error::Unreachable(error.to_string()))?;

        Ok((status, body))
    }
}

/// Turn whatever the person typed into the URL the API answers at.
///
/// A bare host, a base URL, and the full path are all reasonable things to
/// put in a config file, and guessing wrong about which one someone meant is
/// a confusing way to fail. So the path is completed rather than demanded: a
/// host gets `/v1/systemone`, a base already ending in `/v1` gets
/// `/systemone`, and anything already naming the endpoint is left alone.
pub fn resolve_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.ends_with("/systemone") {
        return trimmed.to_string();
    }
    if trimmed.ends_with("/v1") {
        return format!("{trimmed}/systemone");
    }
    format!("{trimmed}/v1/systemone")
}

#[async_trait::async_trait]
impl Decider for SystemOne {
    async fn choose(&self, state: &str, question: &Choice) -> Result<Decision, Error> {
        let criteria: BTreeMap<&str, &str> = question
            .options
            .iter()
            .map(|option| (option.label.as_str(), option.criterion.as_str()))
            .collect();

        let request = Request {
            state,
            model: &self.model,
            questions: BTreeMap::from([(
                QUESTION,
                Question {
                    kind: CHOICE,
                    instructions: &question.instructions,
                    criteria,
                },
            )]),
        };

        let mut attempt = 0;
        let (status, body) = loop {
            let (status, body) = self.attempt(&request).await?;

            if !retryable(status) || attempt >= RETRIES {
                break (status, body);
            }

            attempt += 1;
            let wait = self.retry_delay * attempt;
            tracing::info!(
                status,
                ?wait,
                "the decision model is rate limited; waiting before asking again"
            );
            tokio::time::sleep(wait).await;
        };

        parse_answer(status, &body, question)
    }

    fn name(&self) -> &'static str {
        "system-one"
    }
}

/// Turn an HTTP answer into a decision. Split out from the request so the
/// contract is testable without a service on the other end.
fn parse_answer(status: u16, body: &str, question: &Choice) -> Result<Decision, Error> {
    if !(200..300).contains(&status) {
        return Err(Error::Unusable(format!(
            "the decision model answered HTTP {status}"
        )));
    }

    let parsed: Response = serde_json::from_str(body).map_err(|error| {
        Error::Unusable(format!(
            "the answer was not the JSON eruser expects: {error}"
        ))
    })?;

    let answer = parsed.answers.get(QUESTION).ok_or_else(|| {
        Error::Unusable(format!(
            "the answer did not include the `{QUESTION}` question"
        ))
    })?;

    let choice = answer.choice.trim();
    if choice.is_empty() {
        return Err(Error::Unusable("the model returned no choice".to_string()));
    }
    if !question.offers(choice) {
        return Err(Error::Unusable(format!(
            "the model chose `{choice}`, which was not one of the answers it was offered"
        )));
    }

    // A confidence is the API's to report; when it does not, the probability
    // it gave for the choice it made is the same number said differently.
    // Failing both, the answer stands — eruser asked a closed question, and
    // the caller's own floor is what decides whether that is enough.
    let confidence = answer
        .confidence
        .or_else(|| answer.probabilities.get(choice).copied())
        .unwrap_or(1.0);

    Ok(Decision {
        choice: choice.to_string(),
        confidence,
        probabilities: answer.probabilities.clone(),
        model: parsed.model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question() -> Choice {
        Choice::new(
            "Which reply does this broker's email deserve?",
            [
                Choice::option("missing_info", "they asked for details"),
                Choice::option("confirm", "they asked to confirm"),
                Choice::option("none", "no reply is warranted"),
            ],
        )
    }

    /// A real answer, built from the API's own shape so the fixture cannot
    /// drift from it. `confidence` is `null` when the service does not
    /// report one.
    fn answer(choice: &str, confidence: Option<f32>) -> String {
        serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {
                "decision": {
                    "type": "choice",
                    "choice": choice,
                    "probabilities": {"missing_info": 0.81, "confirm": 0.11},
                    "confidence": confidence,
                }
            }
        })
        .to_string()
    }

    #[test]
    fn the_endpoint_is_completed_from_whatever_was_configured() {
        assert_eq!(
            resolve_endpoint("https://api.typesafe.ai"),
            "https://api.typesafe.ai/v1/systemone"
        );
        assert_eq!(
            resolve_endpoint("https://api.typesafe.ai/"),
            "https://api.typesafe.ai/v1/systemone"
        );
        assert_eq!(
            resolve_endpoint("http://localhost:4000/typesafe"),
            "http://localhost:4000/typesafe/v1/systemone"
        );
        // A base that already names the version gets only the path's tail.
        assert_eq!(
            resolve_endpoint("http://localhost:8000/v1"),
            "http://localhost:8000/v1/systemone"
        );
        // The full path is left exactly as it was.
        assert_eq!(
            resolve_endpoint("http://localhost:8000/v1/systemone"),
            "http://localhost:8000/v1/systemone"
        );
        assert_eq!(
            resolve_endpoint("  https://api.typesafe.ai/v1/systemone/  "),
            "https://api.typesafe.ai/v1/systemone"
        );
    }

    #[test]
    fn a_choice_answer_is_read_as_given() {
        let decision = parse_answer(200, &answer("missing_info", Some(0.82)), &question())
            .expect("a decision");

        assert_eq!(decision.choice, "missing_info");
        assert_eq!(decision.confidence, 0.82);
        assert_eq!(decision.model, "jev-1.13.0");
        assert_eq!(decision.probabilities.get("missing_info"), Some(&0.81));
    }

    #[test]
    fn a_choice_that_was_never_offered_is_refused() {
        let problem = parse_answer(200, &answer("send_them_everything", Some(0.9)), &question())
            .expect_err("an unoffered choice");
        assert!(
            problem.to_string().contains("was not one of the answers"),
            "{problem}"
        );
    }

    #[test]
    fn an_empty_choice_is_refused() {
        assert!(parse_answer(200, &answer("", Some(0.9)), &question()).is_err());
    }

    #[test]
    fn a_failed_request_is_reported_as_unusable_rather_than_a_decision() {
        let problem = parse_answer(500, "boom", &question()).expect_err("a failure");
        assert!(problem.to_string().contains("HTTP 500"), "{problem}");
    }

    #[test]
    fn a_body_that_is_not_the_expected_json_is_refused() {
        assert!(parse_answer(200, "<html>hello</html>", &question()).is_err());
    }

    /// A service that does not report a confidence is not thereby an
    /// untrustworthy one: the probability for the choice it made is the same
    /// number, and the caller's floor applies to it either way.
    #[test]
    fn a_missing_confidence_falls_back_to_the_probability_of_the_choice() {
        let decision =
            parse_answer(200, &answer("missing_info", None), &question()).expect("a decision");
        assert_eq!(decision.confidence, 0.81);

        assert!(decision.accepted_at(0.8).is_some());
        assert!(decision.accepted_at(0.9).is_none());
    }

    #[test]
    fn an_answer_missing_the_question_is_refused() {
        let body = r#"{"model": "jev-1.13.0", "answers": {}}"#;
        assert!(parse_answer(200, body, &question()).is_err());
    }

    #[test]
    fn a_disabled_config_means_no_decider() {
        let config = crate::config::DeciderConfig::default();
        assert!(super::super::from_config(&config).is_none());
    }

    #[test]
    fn an_enabled_config_without_an_endpoint_still_means_no_decider() {
        let config = crate::config::DeciderConfig {
            enabled: true,
            model: "jev-latest".into(),
            ..Default::default()
        };
        assert!(super::super::from_config(&config).is_none());
    }

    #[test]
    fn an_enabled_config_builds_a_client() {
        let config = crate::config::DeciderConfig {
            enabled: true,
            endpoint: "https://api.typesafe.ai".into(),
            model: "jev-latest".into(),
            ..Default::default()
        };
        let decider = super::super::from_config(&config).expect("a configured decider");
        assert_eq!(decider.name(), "system-one");
    }

    // ---------------------------------------------------------------
    // Over a real socket
    // ---------------------------------------------------------------

    /// A server that accepts one connection per reply, records what it was
    /// sent, and answers in order. Enough to exercise the real client without
    /// a model behind it.
    struct Stub {
        endpoint: String,
        requests: tokio::task::JoinHandle<Vec<String>>,
    }

    impl Stub {
        async fn start(replies: Vec<(u16, String)>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a free port");
            let port = listener.local_addr().expect("an address").port();

            let requests = tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;

                let mut seen = Vec::new();
                for (status, body) in replies {
                    let (mut socket, _) = listener.accept().await.expect("a connection");
                    seen.push(read_request(&mut socket).await);

                    let response = format!(
                        "HTTP/1.1 {status} Status\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket
                        .write_all(response.as_bytes())
                        .await
                        .expect("a write");
                    socket.flush().await.expect("a flush");
                }
                seen
            });

            Self {
                endpoint: format!("http://127.0.0.1:{port}"),
                requests,
            }
        }

        async fn requests(self) -> Vec<String> {
            self.requests.await.expect("the server task")
        }
    }

    /// Read one whole request: the head, then as much body as
    /// `Content-Length` promises.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;

        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = socket.read(&mut chunk).await.expect("a read");
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);

            let text = String::from_utf8_lossy(&buffer);
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let promised = head
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if body.len() >= promised {
                    break;
                }
            }
        }

        String::from_utf8_lossy(&buffer).to_string()
    }

    /// The request eruser sends is the one the API documents: the path, the
    /// bearer key, and a `choice` question whose `criteria` are the labels it
    /// is allowed to answer with.
    #[tokio::test]
    async fn the_request_is_the_one_the_api_documents() {
        let stub = Stub::start(vec![(200, answer("missing_info", Some(0.82)))]).await;
        let client = SystemOne::new(
            stub.endpoint.clone(),
            "jev-latest",
            Some("secret-key".to_string()),
            Duration::from_secs(5),
        );

        let decision = client
            .choose("the broker's email", &question())
            .await
            .expect("a decision");
        assert_eq!(decision.choice, "missing_info");
        assert_eq!(decision.model, "jev-1.13.0");

        let requests = stub.requests().await;
        assert_eq!(requests.len(), 1);
        let request = &requests[0];

        assert!(request.starts_with("POST /v1/systemone "), "{request}");
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer secret-key"),
            "{request}"
        );

        let body = request.split("\r\n\r\n").nth(1).expect("a request body");
        let sent: serde_json::Value =
            serde_json::from_str(body).expect("JSON, as the API requires");

        assert_eq!(sent["model"], "jev-latest");
        assert_eq!(sent["state"], "the broker's email");
        assert_eq!(sent["questions"]["decision"]["type"], "choice");
        assert_eq!(
            sent["questions"]["decision"]["instructions"],
            question().instructions
        );
        // The labels eruser will accept, and what each means — the closed
        // vocabulary the whole design rests on.
        assert_eq!(
            sent["questions"]["decision"]["criteria"]["missing_info"],
            "they asked for details"
        );
        assert_eq!(
            sent["questions"]["decision"]["criteria"]["confirm"],
            "they asked to confirm"
        );
    }

    /// The API asks clients to back off on 429 and 529 rather than give up,
    /// and a decision that arrives late is worth more than one that does not.
    #[tokio::test]
    async fn a_rate_limited_answer_is_asked_again() {
        let stub = Stub::start(vec![
            (429, "{}".to_string()),
            (200, answer("confirm", Some(0.9))),
        ])
        .await;
        let client = SystemOne::new(
            stub.endpoint.clone(),
            "jev-latest",
            None,
            Duration::from_secs(5),
        )
        .with_retry_delay(Duration::from_millis(1));

        let decision = client
            .choose("state", &question())
            .await
            .expect("a decision");
        assert_eq!(decision.choice, "confirm");
        assert_eq!(
            stub.requests().await.len(),
            2,
            "asked, waited, and asked again"
        );
    }

    /// A request the API rejected is not going to be accepted on the second
    /// try; asking again would only delay the fallback to the rules.
    #[tokio::test]
    async fn a_rejected_request_is_not_asked_again() {
        let stub = Stub::start(vec![(422, "{}".to_string())]).await;
        let client = SystemOne::new(
            stub.endpoint.clone(),
            "jev-latest",
            None,
            Duration::from_secs(5),
        )
        .with_retry_delay(Duration::from_millis(1));

        let problem = client
            .choose("state", &question())
            .await
            .expect_err("a refusal");
        assert!(problem.to_string().contains("HTTP 422"), "{problem}");
        assert_eq!(stub.requests().await.len(), 1);
    }

    #[test]
    fn only_the_two_statuses_the_api_names_are_retried() {
        assert!(retryable(429), "too many requests");
        assert!(retryable(529), "overloaded");
        assert!(!retryable(422), "a rejected request stays rejected");
        assert!(!retryable(500));
        assert!(!retryable(200));
    }
}
