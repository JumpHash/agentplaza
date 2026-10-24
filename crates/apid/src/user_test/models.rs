//! The models of a user test (PLAN 19.1): the planner (writes the scenarios, judges failures), the driver (a fast picker
//! over the screen's actionable elements, escalating to a stronger model with a screenshot below its threshold) and the
//! checker (looks at checkpoints and at screens with no tree). The loop reads them through [`Models`]; the production
//! impl ([`GatewayModels`]) asks the model gateway, and the tests ask the gateway's `mock` provider with its scripts.
//!
//! Each prompt names its role on its first line (`ROLE: planner plan`, `ROLE: driver tier fast`, …): a script keys on
//! that line and on the goal or screen the prompt names.
//!
//! Vision: a screenshot goes to the model as an image block (its media type and its bytes in base64). The providers whose
//! wire format has no image block refuse such a call, so a step whose checker or escalated driver is one of them fails
//! with the gateway's reason.

use apz_gateway::Gateway;
use apz_proto::chat::{Block, ChatRequest, Message, Role, blocks_text};
use base64::Engine as _;
use serde_json::Value;

use super::scenario::Scenario;
use super::target::{Action, Screenshot};

const SYSTEM: &str = "You test a product the way a careful person would. Answer with one JSON object and nothing else.";

/// The driver's two tiers: the fast picker, and the stronger model it escalates to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Fast,
    Strong,
}

pub struct PlanRequest<'a> {
    pub symbol: &'a str,
    pub acceptance: &'a [String],
    pub regressions: &'a [String],
    pub exploratory: u32,
}

pub struct Judgement {
    /// `true` when the planner reads the failure as a defect of the product; `false` when the scenario or the driver
    /// was wrong.
    pub defect: bool,
    pub reason: String,
}

/// What the driver chooses from: the screen's title and its actionable elements as `(id, description)`.
pub struct PickRequest<'a> {
    pub goal: &'a str,
    pub screen: &'a str,
    pub options: Vec<(String, String)>,
}

#[derive(Debug, PartialEq)]
pub struct Pick {
    /// `None` when the driver's answer is that no option serves the goal.
    pub action: Option<Action>,
    pub confidence: f64,
    pub why: String,
}

pub struct Look {
    pub holds: bool,
    pub reason: String,
}

pub trait Models {
    /// The planner's scenarios for the step: the acceptance ones, and exploratory ones within the budget.
    fn plan(&self, req: &PlanRequest) -> Result<Vec<Scenario>, String>;
    /// The planner's reading of a failed scenario.
    fn judge(&self, scenario: &Scenario, failure: &str) -> Result<Judgement, String>;
    /// The driver's pick. `Tier::Fast` sees the options only; `Tier::Strong` also gets the screenshot.
    fn pick(
        &self,
        tier: Tier,
        req: &PickRequest,
        shot: Option<&Screenshot>,
    ) -> Result<Pick, String>;
    /// The checker's look: does the screenshot show what the question asks.
    fn look(&self, question: &str, shot: &Screenshot) -> Result<Look, String>;
}

/// The models the roles name, as `provider/model` (the recipe's step config).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roles {
    pub planner: String,
    pub driver: String,
    pub escalate: String,
    pub checker: String,
}

/// The production models: the gateway's chats.
pub struct GatewayModels<'a> {
    pub gateway: &'a Gateway,
    pub roles: &'a Roles,
}

impl GatewayModels<'_> {
    /// One answer as JSON: `role` on the first line, then `body`, and the screenshot as an image block when there is one.
    fn ask(
        &self,
        model: &str,
        role: &str,
        body: &str,
        shot: Option<&Screenshot>,
    ) -> Result<Value, String> {
        let mut content = vec![Block::Text {
            text: format!("{role}\n{body}"),
        }];
        if let Some(shot) = shot {
            content.push(Block::Image {
                media_type: shot.mime.clone(),
                data: base64::engine::general_purpose::STANDARD.encode(&shot.bytes),
            });
        }
        let req = ChatRequest {
            model: model.to_string(),
            system: Some(SYSTEM.to_string()),
            system_parts: Vec::new(),
            messages: vec![Message {
                role: Role::User,
                content,
            }],
            tools: Vec::new(),
            cache_last_message: false,
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        let reply = self
            .gateway
            .complete(&req, &mut |_| {})
            .map_err(|e| format!("{model}: {e}"))?;
        json_of(&blocks_text(&reply.content))
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

impl Models for GatewayModels<'_> {
    fn plan(&self, req: &PlanRequest) -> Result<Vec<Scenario>, String> {
        let body = format!(
            "SYMBOL: {}\nACCEPTANCE:\n{}\nREGRESSIONS (must keep passing):\n{}\nEXPLORATORY BUDGET: {}\n\
             Reply {{\"scenarios\": [{{\"id\", \"title\", \"origin\" (acceptance|exploratory), \"symbol\", \
             \"steps\": [{{\"goal\", \"expect\": [{{\"present\"|\"absent\": text}} or \"labelled\"], \"actions\"}}], \
             \"checkpoints\": [{{\"after\", \"question\"}}], \"timings\": [{{\"name\", \"from\", \"to\", \"budget_ms\"}}], \
             \"assertions\": []}}]}}",
            req.symbol,
            bullets(req.acceptance),
            bullets(req.regressions),
            req.exploratory
        );
        let answer = self.ask(&self.roles.planner, "ROLE: planner plan", &body, None)?;
        Scenario::parse_plan(&answer)
    }

    fn judge(&self, scenario: &Scenario, failure: &str) -> Result<Judgement, String> {
        let body = format!(
            "SCENARIO: {}\nFAILURE: {failure}\nReply {{\"verdict\": \"defect\" or \"not_a_defect\", \"reason\": \"...\"}}",
            scenario.title
        );
        let answer = self.ask(&self.roles.planner, "ROLE: planner judge", &body, None)?;
        Ok(Judgement {
            defect: answer["verdict"].as_str() != Some("not_a_defect"),
            reason: answer["reason"].as_str().unwrap_or_default().to_string(),
        })
    }

    fn pick(
        &self,
        tier: Tier,
        req: &PickRequest,
        shot: Option<&Screenshot>,
    ) -> Result<Pick, String> {
        let (model, role) = match tier {
            Tier::Fast => (&self.roles.driver, "ROLE: driver tier fast"),
            Tier::Strong => (&self.roles.escalate, "ROLE: driver tier strong"),
        };
        let options: Vec<String> = req
            .options
            .iter()
            .map(|(id, what)| format!("- {id}: {what}"))
            .collect();
        let body = format!(
            "GOAL: {}\nSCREEN: {}\nOPTIONS:\n{}\nReply {{\"action\": \"click\"|\"type\"|\"press\"|\"none\", \
             \"element\", \"text\", \"key\", \"confidence\": 0 to 1, \"why\"}}",
            req.goal,
            req.screen,
            options.join("\n")
        );
        let answer = self.ask(model, role, &body, shot)?;
        let element = answer["element"].as_str().unwrap_or_default().to_string();
        let action = match answer["action"].as_str() {
            Some("click") => Some(Action::Click {
                element: element.clone(),
            }),
            Some("type") => Some(Action::Type {
                element: element.clone(),
                text: answer["text"].as_str().unwrap_or_default().to_string(),
            }),
            Some("press") => Some(Action::Press {
                key: answer["key"].as_str().unwrap_or_default().to_string(),
            }),
            _ => None,
        };
        // An element the screen does not have is no pick: the driver named something it was not offered.
        let offered = req.options.iter().any(|(id, _)| *id == element);
        let (action, why) = match action {
            Some(_) if !offered => (None, format!("picked {element}, which is not an option")),
            other => (
                other,
                answer["why"].as_str().unwrap_or_default().to_string(),
            ),
        };
        Ok(Pick {
            action,
            confidence: answer["confidence"].as_f64().unwrap_or(0.0),
            why,
        })
    }

    fn look(&self, question: &str, shot: &Screenshot) -> Result<Look, String> {
        let body = format!(
            "QUESTION: {question}\nReply {{\"holds\": true or false, \"reason\": \"...\"}}"
        );
        let answer = self.ask(&self.roles.checker, "ROLE: checker", &body, Some(shot))?;
        Ok(Look {
            holds: answer["holds"].as_bool().unwrap_or(false),
            reason: answer["reason"].as_str().unwrap_or_default().to_string(),
        })
    }
}

fn bullets(items: &[String]) -> String {
    if items.is_empty() {
        return "- (none)".into();
    }
    items
        .iter()
        .map(|i| format!("- {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_json_is_found_in_prose() {
        let v = json_of("Sure. {\"action\": \"click\", \"element\": \"pay\"} Done.").unwrap();
        assert_eq!(v["element"], "pay");
        assert!(json_of("no braces here").is_err());
        assert!(json_of("} backwards {").is_err());
    }
}
