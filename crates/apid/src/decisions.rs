//! Decision models: the plaza's default decision model and the calls that answer typed
//! questions with calibrated probabilities (`Gateway::decide`).
//!
//! GET  /v1/decisions/default  → {provider, model} (nulls when no default is saved)
//! PUT  /v1/decisions/default  body {provider, model} → the same; 400 unless the provider is a
//!                             decision provider that lists the model
//! POST /v1/decisions          body {provider?, model?, input, questions} → {provider, model,
//!                             answers, usage}; provider and model default to the saved one
//!
//! The owner's calls and the agents' (`POST /internal/v1/agents/{cid}/gateway/decisions`, which
//! an agent reaches as `http://plaza.gateway/decisions`) are refused with 409 while STOP ALL holds the plaza. The saved default lives in Postgres under the
//! "decisions_default" key, next to the other plaza state.

use std::sync::Mutex;

use apz_gateway::{DecisionRequest, DecisionResponse, ProviderConfig};
use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::db::Db;
use crate::providers::{Failure, failure, lock};
use crate::state::App;

/// The Postgres key the saved default lives under.
const KEY: &str = "decisions_default";
const HELD: &str = "the plaza is held (STOP ALL); waiting for the owner to resume";

/// A decision model: a provider and one of its models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    pub provider: String,
    pub model: String,
}

/// The saved default. The first read loads it from Postgres; a write saves it there.
#[derive(Default)]
pub struct DefaultStore {
    state: Mutex<DefaultState>,
}

#[derive(Default)]
struct DefaultState {
    loaded: bool,
    pick: Option<Pick>,
}

impl DefaultStore {
    fn get(&self, db: &Db) -> Option<Pick> {
        let mut state = lock(&self.state);
        if !state.loaded {
            state.loaded = true;
            state.pick = db
                .load(KEY)
                .and_then(|text| serde_json::from_str(&text).ok());
        }
        state.pick.clone()
    }

    fn set(&self, pick: Pick) {
        let mut state = lock(&self.state);
        state.loaded = true;
        state.pick = Some(pick);
    }
}

fn current(app: &App) -> Option<Pick> {
    app.providers.decisions.get(&app.db)
}

/// Saves the default in Postgres, then makes it the live one.
fn save(app: &App, pick: &Pick) -> Result<(), Failure> {
    let json = serde_json::to_string(pick).map_err(|e| (500, e.to_string()))?;
    app.db
        .store(KEY, &json)
        .map_err(|e| (500, format!("cannot save: {e}")))?;
    app.providers.decisions.set(pick.clone());
    Ok(())
}

/// The pick is a decision model: a configured provider in the "decisions" group that lists the model.
fn check(providers: &[ProviderConfig], pick: &Pick) -> Result<(), Failure> {
    let provider = providers
        .iter()
        .find(|p| p.id == pick.provider)
        .ok_or_else(|| (400, format!("no provider `{}`", pick.provider)))?;
    if provider.kind.group() != "decisions" {
        return Err((
            400,
            format!(
                "`{}` is a chat provider, not a decision model",
                pick.provider
            ),
        ));
    }
    if !provider.models.contains(&pick.model) {
        return Err((
            400,
            format!(
                "`{}` does not list the model `{}`",
                pick.provider, pick.model
            ),
        ));
    }
    Ok(())
}

/// The pick a call names, with the saved default filling in what it leaves out.
fn resolve(
    default: Option<Pick>,
    provider: Option<String>,
    model: Option<String>,
) -> Result<Pick, Failure> {
    match (provider, model, default) {
        (Some(provider), Some(model), _) => Ok(Pick { provider, model }),
        (None, None, Some(saved)) => Ok(saved),
        (Some(provider), None, Some(saved)) if saved.provider == provider => Ok(saved),
        (None, Some(model), Some(saved)) => Ok(Pick { provider: saved.provider, model }),
        _ => Err((400, "no decision model: name a provider and model, or save a default with PUT /v1/decisions/default".into())),
    }
}

/// Whether a call may run now: the hold, the pick, then the request's shape. Returns the pick.
fn plan(
    paused: bool,
    default: Option<Pick>,
    providers: &[ProviderConfig],
    provider: Option<String>,
    model: Option<String>,
    request: &DecisionRequest,
) -> Result<Pick, Failure> {
    if paused {
        return Err((409, HELD.into()));
    }
    let pick = resolve(default, provider, model)?;
    check(providers, &pick)?;
    request.validate().map_err(|e| (400, e))?;
    Ok(pick)
}

/// One decision call through the gateway. The caller holds no lock while it runs.
fn answer(app: &App, pick: &Pick, request: &DecisionRequest) -> Result<DecisionResponse, Failure> {
    app.providers
        .gateway()
        .decide(&pick.provider, &pick.model, request)
        .map_err(|e| (502, e.to_string()))
}

/// The answer as clients read it: `{provider, model, answers, usage}`.
fn answered(pick: &Pick, resp: &DecisionResponse) -> Value {
    let mut out = serde_json::to_value(resp).unwrap_or(Value::Null);
    out["provider"] = json!(pick.provider);
    out
}

fn default_json(pick: Option<&Pick>) -> Value {
    json!({ "provider": pick.map(|p| p.provider.as_str()), "model": pick.map(|p| p.model.as_str()) })
}

/// `POST /internal/v1/agents/{cid}/gateway/decisions` for agent `cid`: always the saved default. The hold applies, and
/// the call is metered as a model call is: tokens are unpriced, so it costs one credit.
pub(crate) fn agent_decision(app: &App, cid: u64, req: &Request) -> Response {
    if app.agents.get(cid).is_none() {
        return Response::error(404, "unknown agent");
    }
    let request: DecisionRequest = match req.json() {
        Ok(request) => request,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let providers = app.providers.gateway().providers();
    let pick = match plan(
        app.control.paused(),
        current(app),
        &providers,
        None,
        None,
        &request,
    ) {
        Ok(pick) => pick,
        Err(f) => return failure(f),
    };
    let resp = match answer(app, &pick, &request) {
        Ok(resp) => resp,
        Err(f) => return failure(f),
    };
    let work = app.work.current(cid).map_or(0, |item| item.id);
    // pricing: no decision model has a price in the plaza's settings yet, so its tokens cost
    // nothing here and the call costs the one credit a model call costs at least.
    if let Err(message) =
        crate::internal::meter(app, cid, work, 1, &format!("decision {}", pick.model))
    {
        return Response::json(402, &json!({ "error": message }));
    }
    app.transcripts.record(
        &app.bus,
        cid,
        work,
        "decision",
        json!({
            "provider": pick.provider,
            "model": pick.model,
            "questions": request.questions.len(),
            "input_tokens": resp.usage.input_tokens,
        }),
    );
    Response::json(200, &answered(&pick, &resp))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/decisions/default", get_default_route)
        .doc(RouteDoc::new("Get the default decision model"))
        .route("PUT", "/v1/decisions/default", put_default_route)
        .doc(RouteDoc::new("Set the default decision model"))
        .post("/v1/decisions", post_route)
        .doc(RouteDoc::new("Answer decision questions with a model"))
}

fn get_default_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &default_json(current(app).as_ref()))
}

#[derive(Deserialize)]
struct DefaultBody {
    provider: Option<String>,
    model: Option<String>,
}

fn put_default_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body: DefaultBody = match req.json() {
        Ok(body) => body,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    let (Some(provider), Some(model)) = (body.provider, body.model) else {
        return failure((
            400,
            "body must be {\"provider\": \"...\", \"model\": \"...\"}".into(),
        ));
    };
    let pick = Pick { provider, model };
    if let Err(f) = check(&app.providers.gateway().providers(), &pick) {
        return failure(f);
    }
    match save(app, &pick) {
        Ok(()) => Response::json(200, &default_json(Some(&pick))),
        Err(f) => failure(f),
    }
}

#[derive(Deserialize)]
struct PostBody {
    provider: Option<String>,
    model: Option<String>,
    #[serde(flatten)]
    request: DecisionRequest,
}

fn post_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body: PostBody = match req.json() {
        Ok(body) => body,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    let providers = app.providers.gateway().providers();
    let pick = match plan(
        app.control.paused(),
        current(app),
        &providers,
        body.provider,
        body.model,
        &body.request,
    ) {
        Ok(pick) => pick,
        Err(f) => return failure(f),
    };
    let resp = match answer(app, &pick, &body.request) {
        Ok(resp) => resp,
        Err(f) => return failure(f),
    };
    app.bus.publish(
        "decision",
        &json!({
            "provider": pick.provider,
            "model": pick.model,
            "questions": body.request.questions.len(),
            "input_tokens": resp.usage.input_tokens,
        }),
    );
    Response::json(200, &answered(&pick, &resp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_gateway::presets;

    fn preset(id: &str) -> ProviderConfig {
        presets()
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("no preset {id}"))
            .config()
    }

    fn providers() -> Vec<ProviderConfig> {
        vec![preset("openai"), preset("openai-decisions")]
    }

    fn pick(provider: &str, model: &str) -> Pick {
        Pick {
            provider: provider.into(),
            model: model.into(),
        }
    }

    fn request(questions: Value) -> DecisionRequest {
        serde_json::from_value(json!({ "input": "a call from the lobby", "questions": questions }))
            .unwrap()
    }

    fn one_question() -> DecisionRequest {
        request(json!([{ "type": "predicate", "name": "urgent", "instructions": "Is it urgent?" }]))
    }

    #[test]
    fn the_default_must_be_a_decision_model_that_lists_the_model() {
        let providers = providers();
        assert_eq!(
            check(&providers, &pick("openai", "gpt-4o")).unwrap_err().0,
            400,
            "a chat provider is refused"
        );
        assert_eq!(
            check(&providers, &pick("openai-decisions", "not-a-model"))
                .unwrap_err()
                .0,
            400
        );
        assert_eq!(
            check(&providers, &pick("nope", "gpt-6-luna"))
                .unwrap_err()
                .0,
            400
        );
        assert!(check(&providers, &pick("openai-decisions", "gpt-6-luna")).is_ok());
    }

    #[test]
    fn a_call_is_refused_while_held_and_without_a_default() {
        let providers = providers();
        let req = one_question();
        let saved = pick("openai-decisions", "gpt-6-luna");
        assert_eq!(
            plan(true, Some(saved.clone()), &providers, None, None, &req)
                .unwrap_err()
                .0,
            409
        );
        assert_eq!(
            plan(false, None, &providers, None, None, &req)
                .unwrap_err()
                .0,
            400
        );
        assert_eq!(
            plan(false, Some(saved.clone()), &providers, None, None, &req).unwrap(),
            saved
        );
    }

    #[test]
    fn a_call_is_validated_and_names_or_inherits_its_model() {
        let providers = providers();
        let empty = request(json!([]));
        let named = (
            Some("openai-decisions".to_string()),
            Some("gpt-6-luna".to_string()),
        );
        assert_eq!(
            plan(
                false,
                None,
                &providers,
                named.0.clone(),
                named.1.clone(),
                &empty
            )
            .unwrap_err()
            .0,
            400
        );
        let saved = pick("openai-decisions", "gpt-6-luna");
        let inherited = plan(
            false,
            Some(saved.clone()),
            &providers,
            None,
            Some("gpt-6-luna".into()),
            &one_question(),
        );
        assert_eq!(inherited.unwrap(), saved);
        assert_eq!(
            plan(
                false,
                Some(saved),
                &providers,
                Some("openai".into()),
                None,
                &one_question()
            )
            .unwrap_err()
            .0,
            400
        );
    }
}
