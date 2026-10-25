//! Accepting a failure (PLAN 22.10): the planner may accept a failed stage or test only with a recorded reason, and only
//! when a second model agrees. Two model calls, through the gateway's chat path that the other engine steps use: the
//! planner's proposal (`ROLE: qualify accept planner`) and the second model's agreement (`ROLE: qualify accept second`).
//! Either one saying no refuses the acceptance; the answer and the model of each call are kept with it.

use apz_gateway::Gateway;
use apz_proto::chat::{Block, ChatRequest, Message, Role, blocks_text};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The shortest reason an acceptance may give: a word is not a reason.
pub const MIN_REASON: usize = 12;

/// An acceptance the owner or the planner asked for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Request {
    /// The stage that failed (its key).
    pub stage: String,
    /// The test that failed, when the acceptance is of one test (empty: every failure of the stage).
    #[serde(default)]
    pub test: String,
    #[serde(default)]
    pub reason: String,
    /// The planner's model (`provider/model`).
    pub planner: String,
    /// The second model, which must agree (`provider/model`, not the planner's).
    pub second: String,
}

/// An accepted failure, kept with the release's qualification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Acceptance {
    #[table(key)]
    pub id: String,
    pub release: String,
    pub stage: String,
    /// The accepted key: `stage` for the whole stage, or `test:<name>` for one test.
    pub key: String,
    pub reason: String,
    pub planner: String,
    pub planner_says: String,
    pub second: String,
    pub second_says: String,
    /// The tokens the two calls used (the cost of the acceptance).
    pub tokens: u64,
    pub at_ms: u64,
}

/// What one model call answered: its JSON, and the tokens it used.
pub struct Answer {
    pub value: Value,
    pub tokens: u64,
}

/// A model call for an acceptance (production: the gateway; tests: fakes).
pub trait Judge {
    fn ask(&self, model: &str, role: &str, body: &str) -> Result<Answer, String>;
}

/// The production judge: the gateway's chat path, with the JSON object the model answers with.
pub struct GatewayJudge<'a>(pub &'a Gateway);

impl Judge for GatewayJudge<'_> {
    fn ask(&self, model: &str, role: &str, body: &str) -> Result<Answer, String> {
        let req = ChatRequest {
            model: model.to_string(),
            system: Some(
                "You judge a failed release qualification. Answer with one JSON object and nothing else.".into(),
            ),
            system_parts: Vec::new(),
            messages: vec![Message {
                role: Role::User,
                content: vec![Block::Text {
                    text: format!("{role}\n{body}"),
                }],
            }],
            tools: Vec::new(),
            cache_last_message: false,
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        let reply = self
            .0
            .complete(&req, &mut |_| {})
            .map_err(|e| format!("{model}: {e}"))?;
        let text = blocks_text(&reply.content);
        Ok(Answer {
            value: json_of(&text)?,
            tokens: reply.usage.input_tokens + reply.usage.output_tokens,
        })
    }
}

/// The JSON object in a model's answer (the answer may wrap it in prose).
pub fn json_of(text: &str) -> Result<Value, String> {
    let start = text
        .find('{')
        .ok_or("the model's answer holds no JSON object")?;
    let end = text
        .rfind('}')
        .ok_or("the model's answer holds no JSON object")?;
    if end < start {
        return Err("the model's answer holds no JSON object".into());
    }
    serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("the model's answer is not JSON: {e}"))
}

/// The key an acceptance records: `stage`, or `test:<name>` for one test.
pub fn key_of(stage: &str, test: &str) -> String {
    if test.trim().is_empty() {
        stage.to_string()
    } else {
        format!("test:{}", test.trim())
    }
}

/// Judges an acceptance of `failure` (what the stage found) by the two models. The planner proposes it and gives the reason
/// unless the request gives one; the second model must agree. `Ok` only with a proposal, an agreement and a reason of
/// [`MIN_REASON`] characters; otherwise the refusal says which step refused.
pub fn accept(
    judge: &dyn Judge,
    req: &Request,
    failure: &str,
    release: &str,
    id: &str,
    at_ms: u64,
) -> Result<Acceptance, String> {
    if req.planner.trim().is_empty() || req.second.trim().is_empty() {
        return Err("an acceptance names the planner's model and the second model".into());
    }
    if req.planner == req.second {
        return Err("the second model must be a different model from the planner's".into());
    }
    let test = if req.test.trim().is_empty() {
        "(every failure of the stage)"
    } else {
        req.test.trim()
    };
    let body = format!(
        "STAGE: {}\nTEST: {test}\nFAILURE: {failure}\nREASON GIVEN: {}\n\
         Reply {{\"accept\": true or false, \"reason\": \"...\"}}",
        req.stage,
        req.reason.trim()
    );
    let planner = judge.ask(&req.planner, "ROLE: qualify accept planner", &body)?;
    let planner_says = planner.value["reason"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if planner.value["accept"].as_bool() != Some(true) {
        return Err(format!(
            "the planner does not propose the acceptance: {planner_says}"
        ));
    }
    let reason = if req.reason.trim().is_empty() {
        planner_says.clone()
    } else {
        req.reason.trim().to_string()
    };
    if reason.len() < MIN_REASON {
        return Err(format!(
            "an acceptance needs a reason of at least {MIN_REASON} characters"
        ));
    }
    let second_body = format!(
        "{body}\nREASON RECORDED: {reason}\nReply {{\"agree\": true or false, \"reason\": \"...\"}}"
    );
    let second = judge.ask(&req.second, "ROLE: qualify accept second", &second_body)?;
    let second_says = second.value["reason"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if second.value["agree"].as_bool() != Some(true) {
        return Err(format!("the second model does not agree: {second_says}"));
    }
    Ok(Acceptance {
        id: id.to_string(),
        release: release.to_string(),
        stage: req.stage.clone(),
        key: key_of(&req.stage, &req.test),
        reason,
        planner: req.planner.clone(),
        planner_says,
        second: req.second.clone(),
        second_says,
        tokens: planner.tokens + second.tokens,
        at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    /// A judge that answers each role from a fixed reply, and records the models it was asked.
    struct Scripted {
        planner: Value,
        second: Value,
        asked: RefCell<Vec<String>>,
    }

    impl Judge for Scripted {
        fn ask(&self, model: &str, role: &str, _body: &str) -> Result<Answer, String> {
            self.asked.borrow_mut().push(model.to_string());
            let value = if role.contains("planner") {
                self.planner.clone()
            } else {
                self.second.clone()
            };
            Ok(Answer { value, tokens: 100 })
        }
    }

    fn request() -> Request {
        Request {
            stage: "functional".into(),
            test: "cart::pay".into(),
            reason: "the retry is missing in the fixture only".into(),
            planner: "anthropic/planner".into(),
            second: "openai/second".into(),
        }
    }

    #[test]
    fn an_acceptance_needs_a_reason_the_planner_and_a_second_model_that_agrees() {
        let judge = Scripted {
            planner: json!({"accept": true, "reason": "a fixture gap"}),
            second: json!({"agree": true, "reason": "agreed"}),
            asked: RefCell::new(Vec::new()),
        };
        let done = accept(&judge, &request(), "the payment failed", "rel-1", "a1", 5).unwrap();
        assert_eq!(done.key, "test:cart::pay");
        assert_eq!(done.tokens, 200, "both calls are counted");
        assert_eq!(
            *judge.asked.borrow(),
            vec!["anthropic/planner", "openai/second"]
        );
        assert_eq!(done.second_says, "agreed");
    }

    #[test]
    fn a_refused_or_unreasoned_acceptance_is_refused_with_the_step_that_refused() {
        let disagree = Scripted {
            planner: json!({"accept": true, "reason": "a fixture gap"}),
            second: json!({"agree": false, "reason": "the retry is real"}),
            asked: RefCell::new(Vec::new()),
        };
        let err = accept(&disagree, &request(), "f", "rel-1", "a", 0).unwrap_err();
        assert!(err.contains("the second model does not agree"), "{err}");

        let planner_no = Scripted {
            planner: json!({"accept": false, "reason": "a real defect"}),
            second: json!({"agree": true}),
            asked: RefCell::new(Vec::new()),
        };
        let err = accept(&planner_no, &request(), "f", "rel-1", "a", 0).unwrap_err();
        assert!(err.contains("the planner does not propose"), "{err}");
        assert_eq!(
            planner_no.asked.borrow().len(),
            1,
            "the second model is not asked once the planner refuses"
        );

        let mut short = request();
        short.reason = "ok".into();
        let err = accept(&disagree, &short, "f", "rel-1", "a", 0).unwrap_err();
        assert!(err.contains("needs a reason"), "{err}");
        let planner_reason = Scripted {
            planner: json!({"accept": true, "reason": "the fixture drops the retry"}),
            second: json!({"agree": true}),
            asked: RefCell::new(Vec::new()),
        };
        let mut silent = request();
        silent.reason = String::new();
        let taken = accept(&planner_reason, &silent, "f", "rel-1", "a", 0).unwrap();
        assert_eq!(
            taken.reason, "the fixture drops the retry",
            "the planner gives the reason when none is given"
        );

        let mut same = request();
        same.second = same.planner.clone();
        let err = accept(&disagree, &same, "f", "rel-1", "a", 0).unwrap_err();
        assert!(err.contains("different model"), "{err}");
    }

    #[test]
    fn a_stage_wide_acceptance_is_keyed_by_the_stage() {
        assert_eq!(key_of("security", ""), "security");
        assert_eq!(key_of("functional", " cart::pay "), "test:cart::pay");
    }
}
