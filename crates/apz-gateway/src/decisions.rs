//! Decision models: named questions answered with calibrated probabilities,
//! not generated text. One canonical request and response (the OpenAI
//! Decisions shape). OpenAI's API takes it as is; the System One API
//! (TypeSafe Jev, Microsoft Foundry) is translated both ways.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::http::Req;
use crate::{Ctx, GatewayError, ProviderKind};

const OPENAI_BASE: &str = "https://api.openai.com/v1";
const SYSTEMONE_BASE: &str = "https://api.typesafe.ai";
const SYSTEMONE_PATH: &str = "/v1/systemone";

/// One named question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Whether something holds, as a probability.
    Predicate { name: String, instructions: String },
    /// One of `choices`.
    Choice {
        name: String,
        instructions: String,
        choices: Vec<ChoiceOption>,
    },
    /// One of `levels`, lowest first.
    Score {
        name: String,
        instructions: String,
        levels: Vec<Level>,
    },
}

/// A choice's value, with an optional description for the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A score level, with an optional description for the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Question {
    pub fn name(&self) -> &str {
        match self {
            Question::Predicate { name, .. }
            | Question::Choice { name, .. }
            | Question::Score { name, .. } => name,
        }
    }

    pub fn instructions(&self) -> &str {
        match self {
            Question::Predicate { instructions, .. }
            | Question::Choice { instructions, .. }
            | Question::Score { instructions, .. } => instructions,
        }
    }
}

/// The plaza's decision request: an input text and the questions about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub input: String,
    pub questions: Vec<Question>,
}

impl DecisionRequest {
    /// Checks the shape: 1 to 128 questions with unique names and non-empty
    /// instructions; 2 to 255 choices with unique values; 2 to 10 score levels.
    pub fn validate(&self) -> Result<(), String> {
        let count = self.questions.len();
        if !(1..=128).contains(&count) {
            return Err(format!("needs 1 to 128 questions, got {count}"));
        }
        let mut names = HashSet::new();
        for q in &self.questions {
            let name = q.name();
            if !names.insert(name) {
                return Err(format!("question name `{name}` is not unique"));
            }
            if q.instructions().trim().is_empty() {
                return Err(format!("question `{name}` has no instructions"));
            }
            match q {
                Question::Predicate { .. } => {}
                Question::Choice { choices, .. } => {
                    if !(2..=255).contains(&choices.len()) {
                        return Err(format!(
                            "question `{name}` needs 2 to 255 choices, got {}",
                            choices.len()
                        ));
                    }
                    let mut values = HashSet::new();
                    for c in choices {
                        if !values.insert(c.value.as_str()) {
                            return Err(format!(
                                "question `{name}` repeats the choice `{}`",
                                c.value
                            ));
                        }
                    }
                }
                Question::Score { levels, .. } => {
                    if !(2..=10).contains(&levels.len()) {
                        return Err(format!(
                            "question `{name}` needs 2 to 10 levels, got {}",
                            levels.len()
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// One answer, in the request's question order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Predicate {
        name: String,
        probability: f64,
    },
    Choice {
        name: String,
        choice: String,
        probabilities: Vec<ChoiceProbability>,
        confidence: f64,
    },
    /// `score` lies in `0..levels` and may be fractional.
    Score {
        name: String,
        score: f64,
        probabilities: Vec<LevelProbability>,
        confidence: f64,
    },
    /// No answer for this question.
    Refusal {
        name: String,
    },
}

/// The probability of one choice value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceProbability {
    pub value: String,
    pub probability: f64,
}

/// The probability of one score level (by index, with its label).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelProbability {
    pub value: usize,
    pub label: String,
    pub probability: f64,
}

/// Tokens a decision call consumed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u64,
}

/// The answers to a [`DecisionRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    /// The versioned model the upstream reported.
    #[serde(default)]
    pub model: String,
    pub answers: Vec<Answer>,
    #[serde(default)]
    pub usage: Usage,
}

/// Adds the credential (`api-key` for azure-decisions, bearer otherwise) and
/// `extra_headers`.
fn authorize(ctx: &Ctx, req: Req) -> Result<Req, GatewayError> {
    let req = match ctx.credential()? {
        None => req,
        Some(key) if ctx.p.kind == ProviderKind::AzureDecisions => {
            req.secret_header("api-key", &key, &key)
        }
        Some(token) => req.bearer(&token),
    };
    Ok(req.extra_headers(ctx.p.extra_headers.iter()))
}

/// One decision call. `model` is the bare model id; `req` is already
/// validated (the gateway does that before it gets here).
pub(crate) fn decide(
    ctx: &Ctx,
    model: &str,
    req: &DecisionRequest,
) -> Result<DecisionResponse, GatewayError> {
    let (url, body, system_one) = match ctx.p.kind {
        ProviderKind::OpenAiDecisions => (
            format!("{}/decisions", ctx.p.base().unwrap_or(OPENAI_BASE)),
            json!({"model": model, "input": req.input, "questions": req.questions}),
            false,
        ),
        ProviderKind::SystemOne => (
            format!("{}{SYSTEMONE_PATH}", ctx.p.base().unwrap_or(SYSTEMONE_BASE)),
            system_one_body(model, req),
            true,
        ),
        ProviderKind::AzureDecisions => {
            let base = ctx.p.base().ok_or_else(|| {
                GatewayError::Config(format!("provider `{}` needs a base_url", ctx.p.id))
            })?;
            let path = ctx.p.option("path").unwrap_or(SYSTEMONE_PATH);
            (
                format!("{base}/{}", path.trim_start_matches('/')),
                system_one_body(model, req),
                true,
            )
        }
        _ => {
            return Err(GatewayError::Config(format!(
                "provider `{}` is not a decision model",
                ctx.p.id
            )));
        }
    };
    let http_req = authorize(ctx, Req::post_json(url, &ctx.finish_body(body)))?;
    let upstream = ctx.http().json(&http_req)?;
    let mut resp = if system_one {
        from_system_one(model, &req.questions, &upstream)?
    } else {
        serde_json::from_value::<DecisionResponse>(upstream).map_err(GatewayError::decode)?
    };
    if resp.model.is_empty() {
        resp.model = model.to_string();
    }
    Ok(resp)
}

/// The System One body: `input` becomes `state`, the questions a map by name.
fn system_one_body(model: &str, req: &DecisionRequest) -> Value {
    let questions: Map<String, Value> = req
        .questions
        .iter()
        .map(|q| (q.name().to_string(), system_one_question(q)))
        .collect();
    json!({"model": model, "state": req.input, "questions": questions})
}

/// One question in System One's terms: a predicate is a `noul`; a choice's
/// values as a `criteria` map (value to description or null); a score's
/// levels as a `criteria` list of labels.
fn system_one_question(q: &Question) -> Value {
    match q {
        Question::Predicate { instructions, .. } => {
            json!({"type": "noul", "instructions": instructions})
        }
        Question::Choice {
            instructions,
            choices,
            ..
        } => {
            let criteria: Map<String, Value> = choices
                .iter()
                .map(|c| (c.value.clone(), json!(c.description)))
                .collect();
            json!({"type": "choice", "instructions": instructions, "criteria": criteria})
        }
        Question::Score {
            instructions,
            levels,
            ..
        } => {
            let criteria: Vec<&str> = levels.iter().map(|l| l.label.as_str()).collect();
            json!({"type": "score", "instructions": instructions, "criteria": criteria})
        }
    }
}

/// System One's answers (a map by question name) as the list in the request's
/// order. A question without an answer is a refusal.
fn from_system_one(
    model: &str,
    questions: &[Question],
    upstream: &Value,
) -> Result<DecisionResponse, GatewayError> {
    let answers = upstream
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| GatewayError::decode("System One answers are not an object"))?;
    let mut list = Vec::with_capacity(questions.len());
    for q in questions {
        list.push(match answers.get(q.name()) {
            Some(value) if !value.is_null() => one_system_one_answer(q, value)?,
            _ => Answer::Refusal {
                name: q.name().to_string(),
            },
        });
    }
    Ok(DecisionResponse {
        model: upstream
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_string(),
        answers: list,
        usage: Usage {
            input_tokens: upstream
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
    })
}

/// One System One answer, read against its question's shape.
fn one_system_one_answer(q: &Question, value: &Value) -> Result<Answer, GatewayError> {
    let name = q.name();
    let number = |v: Option<&Value>, what: &str| {
        v.and_then(Value::as_f64)
            .ok_or_else(|| GatewayError::decode(format!("answer `{name}`: {what} is not a number")))
    };
    Ok(match q {
        Question::Predicate { .. } => Answer::Predicate {
            name: name.to_string(),
            probability: number(value.get("noul"), "noul")?,
        },
        Question::Choice { choices, .. } => {
            let probs = probabilities(value, name)?;
            let mut list = Vec::with_capacity(choices.len());
            for c in choices {
                let probability = number(
                    probs.get(&c.value),
                    &format!("probability of `{}`", c.value),
                )?;
                list.push(ChoiceProbability {
                    value: c.value.clone(),
                    probability,
                });
            }
            let choice = value
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::decode(format!("answer `{name}` has no choice")))?;
            Answer::Choice {
                name: name.to_string(),
                choice: choice.to_string(),
                probabilities: list,
                confidence: number(value.get("confidence"), "confidence")?,
            }
        }
        Question::Score { levels, .. } => {
            let probs = probabilities(value, name)?;
            let mut list = Vec::with_capacity(levels.len());
            for (i, level) in levels.iter().enumerate() {
                let probability = number(
                    probs.get(&i.to_string()),
                    &format!("probability of level {i}"),
                )?;
                list.push(LevelProbability {
                    value: i,
                    label: level.label.clone(),
                    probability,
                });
            }
            Answer::Score {
                name: name.to_string(),
                score: number(value.get("score"), "score")?,
                probabilities: list,
                confidence: number(value.get("confidence"), "confidence")?,
            }
        }
    })
}

/// The `probabilities` map of one System One answer.
fn probabilities<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, GatewayError> {
    value
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| GatewayError::decode(format!("answer `{name}` has no probabilities")))
}

#[cfg(test)]
mod tests {
    use apz_proto::chat::ChatRequest;

    use super::*;
    use crate::Auth;
    use crate::testutil::{FakeServer, Reply, gateway, provider, request};

    fn sample() -> DecisionRequest {
        DecisionRequest {
            input: "Customer asks for a refund after 40 days.".into(),
            questions: vec![
                Question::Predicate {
                    name: "refund".into(),
                    instructions: "Is a refund due?".into(),
                },
                Question::Choice {
                    name: "route".into(),
                    instructions: "Where does it go?".into(),
                    choices: vec![
                        ChoiceOption {
                            value: "billing".into(),
                            description: Some("money questions".into()),
                        },
                        ChoiceOption {
                            value: "support".into(),
                            description: None,
                        },
                    ],
                },
                Question::Score {
                    name: "urgency".into(),
                    instructions: "How urgent?".into(),
                    levels: vec![
                        Level {
                            label: "low".into(),
                            description: None,
                        },
                        Level {
                            label: "high".into(),
                            description: None,
                        },
                    ],
                },
            ],
        }
    }

    #[test]
    fn validation_rejects_bad_requests() {
        assert!(sample().validate().is_ok());
        let mut none = sample();
        none.questions.clear();
        assert!(none.validate().unwrap_err().contains("1 to 128"));
        let mut many = sample();
        many.questions = (0..129)
            .map(|i| Question::Predicate {
                name: format!("q{i}"),
                instructions: "x".into(),
            })
            .collect();
        assert!(many.validate().unwrap_err().contains("1 to 128"));
        let mut dup = sample();
        dup.questions.push(Question::Predicate {
            name: "refund".into(),
            instructions: "again".into(),
        });
        assert!(dup.validate().unwrap_err().contains("not unique"));
        let mut blank = sample();
        blank.questions[0] = Question::Predicate {
            name: "refund".into(),
            instructions: "  ".into(),
        };
        assert!(blank.validate().unwrap_err().contains("no instructions"));
        let mut one_choice = sample();
        if let Question::Choice { choices, .. } = &mut one_choice.questions[1] {
            choices.truncate(1);
        }
        assert!(one_choice.validate().unwrap_err().contains("2 to 255"));
        let mut repeated = sample();
        if let Question::Choice { choices, .. } = &mut repeated.questions[1] {
            choices[1].value = "billing".into();
        }
        assert!(repeated.validate().unwrap_err().contains("repeats"));
        let mut tall = sample();
        if let Question::Score { levels, .. } = &mut tall.questions[2] {
            *levels = (0..11)
                .map(|i| Level {
                    label: i.to_string(),
                    description: None,
                })
                .collect();
        }
        assert!(tall.validate().unwrap_err().contains("2 to 10"));
    }

    #[test]
    fn openai_decisions_send_the_canonical_body_and_decode_the_answers() {
        let reply = json!({
            "model": "gpt-6-luna-2026-09",
            "answers": [
                {"type": "predicate", "name": "refund", "probability": 0.91},
                {"type": "choice", "name": "route", "choice": "billing",
                 "probabilities": [{"value": "billing", "probability": 0.8}, {"value": "support", "probability": 0.2}],
                 "confidence": 0.7},
                {"type": "score", "name": "urgency", "score": 0.4,
                 "probabilities": [{"value": 0, "label": "low", "probability": 0.6},
                                   {"value": 1, "label": "high", "probability": 0.4}],
                 "confidence": 0.5},
            ],
            "usage": {"input_tokens": 42}
        });
        let server = FakeServer::start(vec![Reply::json(200, &reply)]);
        let gw = gateway(vec![provider(
            "oa",
            ProviderKind::OpenAiDecisions,
            &server,
            Auth::Env("OPENAI_API_KEY".into()),
        )]);
        let req = sample();
        let resp = gw.decide("oa", "gpt-6-luna", &req).unwrap();
        assert_eq!(resp.model, "gpt-6-luna-2026-09");
        assert_eq!(resp.usage.input_tokens, 42);
        assert_eq!(
            resp.answers[0],
            Answer::Predicate {
                name: "refund".into(),
                probability: 0.91
            }
        );
        assert!(matches!(&resp.answers[1], Answer::Choice { choice, .. } if choice == "billing"));
        let sent = &server.requests()[0];
        assert_eq!(sent.path, "/decisions");
        assert_eq!(
            sent.header("authorization"),
            Some("Bearer env-OPENAI_API_KEY")
        );
        assert_eq!(
            sent.json(),
            json!({"model": "gpt-6-luna", "input": req.input, "questions": req.questions})
        );
    }

    #[test]
    fn systemone_translates_the_request_and_maps_answers_back_in_order() {
        let reply = json!({
            "model": "jev-1.13.0",
            "answers": {
                "route": {"choice": "support", "probabilities": {"support": 0.6, "billing": 0.4}, "confidence": 0.9},
                "urgency": {"score": 1.5, "probabilities": {"0": 0.3, "1": 0.7}, "confidence": 0.8}
            },
            "usage": {"input_tokens": 7}
        });
        let server = FakeServer::start(vec![Reply::json(200, &reply)]);
        let gw = gateway(vec![provider(
            "ts",
            ProviderKind::SystemOne,
            &server,
            Auth::Env("TYPESAFE_API_KEY".into()),
        )]);
        let resp = gw.decide("ts", "jev-latest", &sample()).unwrap();
        assert_eq!(resp.model, "jev-1.13.0");
        assert_eq!(resp.usage.input_tokens, 7);
        assert_eq!(
            resp.answers[0],
            Answer::Refusal {
                name: "refund".into()
            },
            "a missing answer is a refusal"
        );
        assert_eq!(
            resp.answers[1],
            Answer::Choice {
                name: "route".into(),
                choice: "support".into(),
                probabilities: vec![
                    ChoiceProbability {
                        value: "billing".into(),
                        probability: 0.4
                    },
                    ChoiceProbability {
                        value: "support".into(),
                        probability: 0.6
                    },
                ],
                confidence: 0.9,
            }
        );
        assert_eq!(
            resp.answers[2],
            Answer::Score {
                name: "urgency".into(),
                score: 1.5,
                probabilities: vec![
                    LevelProbability {
                        value: 0,
                        label: "low".into(),
                        probability: 0.3
                    },
                    LevelProbability {
                        value: 1,
                        label: "high".into(),
                        probability: 0.7
                    },
                ],
                confidence: 0.8,
            }
        );
        let sent = &server.requests()[0];
        assert_eq!(sent.path, "/v1/systemone");
        assert_eq!(
            sent.header("authorization"),
            Some("Bearer env-TYPESAFE_API_KEY")
        );
        let body = sent.json();
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"], "Customer asks for a refund after 40 days.");
        assert_eq!(
            body["questions"]["refund"],
            json!({"type": "noul", "instructions": "Is a refund due?"})
        );
        assert_eq!(
            body["questions"]["route"],
            json!({"type": "choice", "instructions": "Where does it go?", "criteria": {"billing": "money questions", "support": null}})
        );
        assert_eq!(
            body["questions"]["urgency"],
            json!({"type": "score", "instructions": "How urgent?", "criteria": ["low", "high"]})
        );
    }

    #[test]
    fn azure_sends_the_api_key_header_and_honours_the_path_option() {
        let reply =
            json!({"model": "Microsoft-Decision-1", "answers": {}, "usage": {"input_tokens": 3}});
        let server = FakeServer::start(vec![Reply::json(200, &reply), Reply::json(200, &reply)]);
        let mut az = provider(
            "az",
            ProviderKind::AzureDecisions,
            &server,
            Auth::Secret("s".into()),
        );
        let gw = gateway(vec![az.clone()]);
        let resp = gw.decide("az", "Microsoft-Decision-1", &sample()).unwrap();
        assert!(
            resp.answers
                .iter()
                .all(|a| matches!(a, Answer::Refusal { .. }))
        );
        let sent = &server.requests()[0];
        assert_eq!(sent.path, "/v1/systemone");
        assert_eq!(sent.header("api-key"), Some("sk-secret-value"));
        assert_eq!(sent.header("authorization"), None);
        az.options = json!({"path": "/custom/decide"});
        gateway(vec![az])
            .decide("az", "Microsoft-Decision-1", &sample())
            .unwrap();
        assert_eq!(server.requests()[1].path, "/custom/decide");
    }

    #[test]
    fn upstream_errors_keep_their_kind_and_never_carry_the_key() {
        let server = FakeServer::start(vec![Reply::new(
            401,
            "application/json",
            r#"{"error":"bad key sk-secret-value"}"#,
        )]);
        let gw = gateway(vec![provider(
            "ts",
            ProviderKind::SystemOne,
            &server,
            Auth::Secret("s".into()),
        )]);
        match gw.decide("ts", "jev-latest", &sample()).unwrap_err() {
            GatewayError::Http {
                status,
                body,
                retryable,
            } => {
                assert_eq!((status, retryable), (401, false));
                assert!(!body.contains("sk-secret-value"), "{body}");
            }
            other => panic!("expected an HTTP error, got {other:?}"),
        }
    }

    #[test]
    fn decision_and_chat_kinds_do_not_mix() {
        let server = FakeServer::start(vec![]);
        let gw = gateway(vec![
            provider("ts", ProviderKind::SystemOne, &server, Auth::None),
            provider("ch", ProviderKind::Mock, &server, Auth::None),
        ]);
        let chat = gw.complete(
            &ChatRequest {
                model: "ts/jev-latest".into(),
                ..request("hi")
            },
            &mut |_| {},
        );
        assert_eq!(
            chat.unwrap_err(),
            GatewayError::Config("ts is a decision model; it answers questions, not chats".into())
        );
        let not_decision = gw.decide("ch", "echo", &sample()).unwrap_err();
        assert!(
            matches!(&not_decision, GatewayError::Config(m) if m.contains("chat model")),
            "{not_decision:?}"
        );
        let empty = DecisionRequest {
            input: "x".into(),
            questions: Vec::new(),
        };
        assert!(matches!(
            gw.decide("ts", "jev-latest", &empty),
            Err(GatewayError::Config(_))
        ));
        assert!(server.requests().is_empty());
    }
}
