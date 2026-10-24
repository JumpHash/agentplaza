//! The login bridge's switch (PLAN 11.2; Engine 13.3). The bridge would let the hub, on the owner's machine, read a vendor
//! CLI's login there and call the vendor's endpoints with it. This module builds only the switch: one per vendor, off
//! by default, labelled with the vendor's terms, and the vault binding the bridge would read its login from. No login
//! is read, stored or used here. Whether a vendor's terms allow it is the owner's to decide (PLAN 28.1 point 2), so
//! the switch is offered to the hub's owner only, and [`checked`] refuses every other caller and every vendor whose
//! switch is off.
//!
//! The switches are rows of plaza.db (table `login_bridge`); a vendor with no row is off.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::hub_auth::is_owner;
use crate::providers::failure;
use crate::state::App;
use crate::store::saver::save_now;
use crate::vault::name_for;

/// The vendors the switch is offered for, each with its terms as the switch labels them.
pub const VENDORS: [(&str, &str); 2] = [
    (
        "anthropic",
        "Anthropic's terms: its Agent SDK documentation says third-party products may not offer claude.ai login or its \
         rate limits without approval. The owner decides this (PLAN 28.1 point 2).",
    ),
    (
        "openai",
        "OpenAI's terms: a ChatGPT subscription is licensed for OpenAI's own apps. Check OpenAI's current terms before \
         turning this on. The owner decides this (PLAN 28.1 point 2).",
    ),
];

/// One vendor's switch (table `login_bridge`, keyed by the vendor).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "login_bridge")]
pub struct Switch {
    #[table(key)]
    pub vendor: String,
    pub enabled: bool,
    pub updated_ms: u64,
}

/// The switches as the hub holds them.
#[derive(Default)]
pub struct Store {
    switches: Mutex<BTreeMap<String, Switch>>,
}

impl Store {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Switch>> {
        self.switches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The switches' rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(def(), self.lock().values())
    }

    /// Replaces the switches with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<Switch> = apz_store::table::load_list(conn, def())?;
        *self.lock() = stored.into_iter().map(|s| (s.vendor.clone(), s)).collect();
        Ok(())
    }

    /// Whether `vendor`'s switch is on. A vendor with no row is off.
    pub fn enabled(&self, vendor: &str) -> bool {
        self.lock().get(vendor).is_some_and(|s| s.enabled)
    }

    /// Sets `vendor`'s switch, and returns the row it replaced.
    fn put(&self, vendor: &str, enabled: bool, now: u64) -> Option<Switch> {
        self.lock().insert(
            vendor.to_string(),
            Switch {
                vendor: vendor.to_string(),
                enabled,
                updated_ms: now,
            },
        )
    }

    /// Puts back the row [`Store::put`] replaced (a save that failed).
    fn restore(&self, vendor: &str, before: Option<Switch>) {
        let mut switches = self.lock();
        match before {
            Some(row) => switches.insert(vendor.to_string(), row),
            None => switches.remove(vendor),
        };
    }
}

/// The login bridge's table: `login_bridge`, one row per vendor that was switched.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Switch)
}

/// The vault binding the bridge would read a vendor's login from. Nothing is stored under it yet.
pub fn binding(vendor: &str) -> String {
    format!("login-bridge:{vendor}")
}

/// The vault name of the login the bridge would read for a vendor.
pub fn vault_name(vendor: &str) -> String {
    name_for(&format!("login-bridge-{vendor}-login"))
}

/// What a use of the bridge needs from the vault: the name it reads and the binding it reads under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub name: String,
    pub binding: String,
}

/// Why the bridge may not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The caller is not the hub's owner.
    NotOwner,
    /// The vendor is not one the switch is offered for.
    UnknownVendor,
    /// The vendor's switch is off (the default).
    Off,
}

/// Whether the bridge may read a vendor's login for `owner`: the owner asks, the vendor is offered, and its switch is on.
/// Only then does the caller learn where the login would be read from.
pub fn checked(store: &Store, owner: bool, vendor: &str) -> Result<Reading, Refusal> {
    if !owner {
        return Err(Refusal::NotOwner);
    }
    if !VENDORS.iter().any(|(v, _)| *v == vendor) {
        return Err(Refusal::UnknownVendor);
    }
    if !store.enabled(vendor) {
        return Err(Refusal::Off);
    }
    Ok(Reading {
        name: vault_name(vendor),
        binding: binding(vendor),
    })
}

fn terms_of(vendor: &str) -> Option<&'static str> {
    VENDORS.iter().find(|(v, _)| *v == vendor).map(|(_, t)| *t)
}

/// A vendor's switch as the owner sees it: whether the bridge would be usable for it now, and if not, why.
fn view(app: &App, vendor: &str, terms: &str) -> Value {
    let check = checked(&app.login_bridges, true, vendor);
    json!({
        "vendor": vendor,
        "enabled": app.login_bridges.enabled(vendor),
        "terms": terms,
        "binding": binding(vendor),
        "vault_name": vault_name(vendor),
        "usable": check.is_ok(),
        "refusal": check.as_ref().err().map(|r| format!("{r:?}")),
    })
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/login-bridges", list_route)
        .doc(
            RouteDoc::new(
                "The login bridge's switch per vendor: off by default, labelled with the vendor's terms (the owner only)",
            )
            .errors(&[403]),
        )
        .route("PUT", "/v1/login-bridges/{vendor}", set_route)
        .doc(
            RouteDoc::new("Turn a vendor's login bridge on or off: {enabled} (the owner only)")
                .errors(&[400, 403, 404, 500]),
        )
}

fn refused() -> Response {
    failure((
        403,
        "the login bridge is the hub's owner's to switch".into(),
    ))
}

fn list_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    if !is_owner(app, req) {
        return refused();
    }
    let rows: Vec<Value> = VENDORS
        .iter()
        .map(|(vendor, terms)| view(app, vendor, terms))
        .collect();
    Response::json(200, &rows)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    enabled: bool,
}

fn set_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    if !is_owner(app, req) {
        return refused();
    }
    let vendor = p.get("vendor");
    let Some(terms) = terms_of(vendor) else {
        return failure((404, format!("no login bridge for `{vendor}`")));
    };
    let body: Set = match req.json() {
        Ok(b) => b,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    let before = app
        .login_bridges
        .put(vendor, body.enabled, apz_plaza::now_ms());
    if !save_now(app, "login_bridges") {
        app.login_bridges.restore(vendor, before);
        return failure((
            500,
            "write the login bridges: the store did not save them".into(),
        ));
    }
    Response::json(200, &view(app, vendor, terms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vendor_with_no_switch_is_off_and_refuses() {
        let store = Store::default();
        assert_eq!(checked(&store, true, "anthropic"), Err(Refusal::Off));
        assert_eq!(checked(&store, true, "openai"), Err(Refusal::Off));
    }

    #[test]
    fn only_the_owner_gets_past_a_switch_that_is_on() {
        let store = Store::default();
        store.put("anthropic", true, 1);
        assert_eq!(checked(&store, false, "anthropic"), Err(Refusal::NotOwner));
        let reading = checked(&store, true, "anthropic").expect("on, for the owner");
        assert_eq!(reading.binding, "login-bridge:anthropic");
        assert_eq!(reading.name, vault_name("anthropic"));
        assert_eq!(checked(&store, true, "openai"), Err(Refusal::Off));
    }

    #[test]
    fn an_unoffered_vendor_is_refused_even_when_switched_on() {
        let store = Store::default();
        store.put("example", true, 1);
        assert_eq!(
            checked(&store, true, "example"),
            Err(Refusal::UnknownVendor)
        );
    }

    #[test]
    fn switching_off_again_refuses_again_and_a_failed_save_restores_the_row() {
        let store = Store::default();
        store.put("openai", true, 1);
        let before = store.put("openai", false, 2);
        assert_eq!(checked(&store, true, "openai"), Err(Refusal::Off));
        store.restore("openai", before);
        assert!(store.enabled("openai"));
    }

    #[test]
    fn every_vendor_switch_labels_the_terms_and_the_plan_point() {
        for (vendor, terms) in VENDORS {
            assert!(terms.contains("PLAN 28.1 point 2"), "{vendor}: {terms}");
        }
    }
}
