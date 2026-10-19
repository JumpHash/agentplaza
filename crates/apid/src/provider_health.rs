//! The providers' health: one check per provider (its key, then its model listing and how long the listing
//! took), run by hand (`POST /v1/providers/check`, `POST /v1/providers/{id}/check`) and in the background
//! every `health_interval_s` seconds. Each round is published on the bus as `providers.health`, so the
//! consoles show it without polling. A check never makes a paid call: listing models is free.

use std::sync::Arc;
use std::time::{Duration, Instant};

use apz_gateway::{Caller, Gateway, ProviderConfig};
use serde_json::{Value, json};

use crate::state::App;

/// How long the background loop waits after a start before its first round.
const FIRST_ROUND: Duration = Duration::from_secs(15);
/// How often the loop looks at the interval (so a changed setting applies within this).
const TICK: Duration = Duration::from_secs(5);

/// One check of `p`, given its credential state (`credential` in the provider view): `ok`, `warn` or `crit`
/// with a sentence for the consoles, the time, the listing's latency and how many models it offered.
pub fn check(gateway: &Gateway, p: &ProviderConfig, credential: &str) -> Value {
    let checked_ms = apz_plaza::now_ms();
    let refused = match credential {
        "missing" => Some(("crit", "Not authenticated · add a key")),
        "sealed" => Some(("crit", "The vault is sealed · unseal it to use this key")),
        "out_of_scope" => Some(("warn", "Its key is held only by other nodes")),
        _ => None,
    };
    if let Some((status, message)) = refused {
        return json!({ "status": status, "message": message, "checked_ms": checked_ms });
    }
    let started = Instant::now();
    let listed = gateway.list_models_for(&p.id, &Caller::Owner { node: None });
    let latency_ms = started.elapsed().as_millis() as u64;
    match listed {
        Ok(models) if models.is_empty() => json!({
            "status": "warn",
            "message": "Reachable · it lists no models",
            "checked_ms": checked_ms,
            "latency_ms": latency_ms,
            "models": 0,
        }),
        Ok(models) => json!({
            "status": "ok",
            "message": format!("Ready · {} {}", models.len(), if models.len() == 1 { "model" } else { "models" }),
            "checked_ms": checked_ms,
            "latency_ms": latency_ms,
            "models": models.len(),
        }),
        Err(err) => json!({
            "status": "crit",
            "message": failure_sentence(&err.to_string()),
            "checked_ms": checked_ms,
            "latency_ms": latency_ms,
        }),
    }
}

/// A gateway error as the consoles say it: an auth refusal as "Not authenticated", the rest as it is.
fn failure_sentence(err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    if lower.contains("http 401") || lower.contains("http 403") || lower.contains("credentials") {
        format!("Not authenticated · {}", short(err))
    } else if lower.contains("transport") {
        format!("Not reachable · {}", short(err))
    } else {
        short(err)
    }
}

/// At most 160 characters of `text`, on one line.
fn short(text: &str) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 160 {
        format!("{}…", line.chars().take(159).collect::<String>())
    } else {
        line
    }
}

/// Starts the background checks: a round every `health_interval_s` seconds (none while it is 0), the first
/// shortly after the start.
pub fn spawn(app: Arc<App>) {
    std::thread::Builder::new()
        .name("provider-health".into())
        .spawn(move || {
            std::thread::sleep(FIRST_ROUND);
            let mut last: Option<Instant> = None;
            loop {
                let interval = app.settings.get().health_interval_s;
                if due(interval, last.map(|at| at.elapsed())) {
                    let health = app.providers.check_all();
                    app.bus.publish("providers.health", &health);
                    // The engines after the providers: an engine's health reads its route's provider.
                    let engines = crate::engine_instances::check_now(&app);
                    app.bus.publish("engines.health", &engines);
                    last = Some(Instant::now());
                }
                std::thread::sleep(TICK);
            }
        })
        .expect("spawn the provider health thread");
}

/// Whether a round is due: never while `interval_s` is 0, at once before the first round, else once
/// `interval_s` has passed since the last.
fn due(interval_s: u32, since_last: Option<Duration>) -> bool {
    match (interval_s, since_last) {
        (0, _) => false,
        (_, None) => true,
        (interval, Some(since)) => since >= Duration::from_secs(u64::from(interval)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_gateway::ProviderKind;

    #[test]
    fn rounds_follow_the_interval() {
        assert!(!due(0, None));
        assert!(!due(0, Some(Duration::from_secs(1_000_000))));
        assert!(due(300, None));
        assert!(!due(300, Some(Duration::from_secs(299))));
        assert!(due(300, Some(Duration::from_secs(300))));
    }

    #[test]
    fn a_missing_key_is_not_authenticated_and_makes_no_call() {
        let gw = Gateway::builder().providers(vec![]).build();
        let p = ProviderConfig::new("openai", ProviderKind::OpenAi);
        let h = check(&gw, &p, "missing");
        assert_eq!(h["status"], "crit");
        assert!(
            h["message"]
                .as_str()
                .unwrap()
                .starts_with("Not authenticated")
        );
        assert!(h.get("latency_ms").is_none());
    }

    #[test]
    fn failures_read_as_sentences() {
        assert!(
            failure_sentence("HTTP 401: {\"error\":\"bad key\"}").starts_with("Not authenticated")
        );
        assert!(failure_sentence("transport: connection refused").starts_with("Not reachable"));
        assert_eq!(short(&"a ".repeat(200)).chars().count(), 160);
    }
}
