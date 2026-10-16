//! What a route tells its API's OpenAPI spec (`apid openapi`).
//!
//! A route carries its documentation next to its handler, so the spec is read off the router and its
//! paths cannot drift from what the server answers:
//!
//! ```ignore
//! router
//!     .get("/v1/agents", list_agents)
//!     .doc(RouteDoc::new("List the agents").response::<Vec<AgentView>>())
//! ```
//!
//! A route without a doc is still in the spec (method, path, parameters, auth), marked undocumented.

use schemars::{JsonSchema, Schema, SchemaGenerator};

/// Builds one type's JSON Schema in a generator (which collects the named types it refers to).
pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

/// One route's documentation.
#[derive(Clone, Debug, Default)]
pub struct RouteDoc {
    /// One line: what the call does ("List the agents").
    pub summary: &'static str,
    /// More, when one line is not enough (Markdown).
    pub description: &'static str,
    /// The group it is listed under. None: the path's first segment after `/v1/` (`agents`).
    pub tag: Option<&'static str>,
    /// The JSON request body's schema, for a route that takes one.
    pub request: Option<SchemaFn>,
    /// The JSON success body's schema.
    pub response: Option<SchemaFn>,
    /// The success status, when not 200 (201 Created, 202 Accepted, 204 No Content).
    pub status: Option<u16>,
    /// The error statuses it answers besides the router's own (400 bad path, 401 unauthenticated).
    pub errors: &'static [u16],
    /// The response is a server-sent event stream (`text/event-stream`), not JSON.
    pub stream: bool,
}

impl RouteDoc {
    pub fn new(summary: &'static str) -> Self {
        Self {
            summary,
            ..Self::default()
        }
    }

    pub fn description(mut self, text: &'static str) -> Self {
        self.description = text;
        self
    }

    pub fn tag(mut self, tag: &'static str) -> Self {
        self.tag = Some(tag);
        self
    }

    pub fn request<T: JsonSchema>(mut self) -> Self {
        self.request = Some(|generator| generator.subschema_for::<T>());
        self
    }

    pub fn response<T: JsonSchema>(mut self) -> Self {
        self.response = Some(|generator| generator.subschema_for::<T>());
        self
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn errors(mut self, statuses: &'static [u16]) -> Self {
        self.errors = statuses;
        self
    }

    pub fn stream(mut self) -> Self {
        self.stream = true;
        self
    }
}

/// One route as the router holds it, for a spec generator.
#[derive(Clone, Copy, Debug)]
pub struct RouteInfo<'a> {
    pub method: &'static str,
    /// As registered: `/v1/agents/{cid}/tell`.
    pub pattern: &'static str,
    pub doc: Option<&'a RouteDoc>,
}

impl RouteInfo<'_> {
    /// The `{name}` parameters in the pattern, in order.
    pub fn params(&self) -> impl Iterator<Item = &'static str> {
        self.pattern
            .split('/')
            .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
    }
}
