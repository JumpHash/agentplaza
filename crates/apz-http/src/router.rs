//! Method + path-pattern dispatch.

use crate::{Request, Response};

type Handler<S> = Box<dyn Fn(&S, &Request, &Params<'_>) -> Response + Send + Sync>;

/// A table of routes over shared state `S`. Patterns are `/`-separated
/// segments; `{name}` captures one non-empty segment. The first match wins; a
/// path that matches only under other methods is a 405, no match a 404.
pub struct Router<S> {
    routes: Vec<Route<S>>,
}

struct Route<S> {
    method: &'static str,
    pattern: Vec<&'static str>,
    handler: Handler<S>,
}

/// The segments a pattern's `{name}`s captured.
#[derive(Debug, Default)]
pub struct Params<'a> {
    pairs: Vec<(&'static str, &'a str)>,
}

impl Params<'_> {
    /// The segment captured as `{name}`; empty when the pattern has none.
    pub fn get(&self, name: &str) -> &str {
        self.pairs.iter().find(|(key, _)| *key == name).map_or("", |(_, value)| value)
    }
}

impl<S> Default for Router<S> {
    fn default() -> Self {
        Self { routes: Vec::new() }
    }
}

impl<S> Router<S> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn route(
        mut self,
        method: &'static str,
        pattern: &'static str,
        handler: impl Fn(&S, &Request, &Params<'_>) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.routes.push(Route {
            method,
            pattern: pattern.trim_start_matches('/').split('/').collect(),
            handler: Box::new(handler),
        });
        self
    }

    pub fn get(
        self,
        pattern: &'static str,
        handler: impl Fn(&S, &Request, &Params<'_>) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.route("GET", pattern, handler)
    }

    pub fn post(
        self,
        pattern: &'static str,
        handler: impl Fn(&S, &Request, &Params<'_>) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.route("POST", pattern, handler)
    }

    pub fn delete(
        self,
        pattern: &'static str,
        handler: impl Fn(&S, &Request, &Params<'_>) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.route("DELETE", pattern, handler)
    }

    /// Answers `request` from the first matching route.
    pub fn handle(&self, state: &S, request: &Request) -> Response {
        let mut allowed: Vec<&str> = Vec::new();
        for route in &self.routes {
            let Some(params) = matches(&route.pattern, &request.path) else {
                continue;
            };
            if route.method == request.method {
                return (route.handler)(state, request, &params);
            }
            if !allowed.contains(&route.method) {
                allowed.push(route.method);
            }
        }
        if allowed.is_empty() {
            return Response::error(404, "no such endpoint");
        }
        let allowed = allowed.join(", ");
        Response::error(405, &format!("this endpoint accepts {allowed}")).with_header("Allow", &allowed)
    }
}

fn matches<'a>(pattern: &[&'static str], path: &'a str) -> Option<Params<'a>> {
    let mut segments = path.strip_prefix('/')?.split('/');
    let mut params = Params::default();
    for want in pattern {
        let got = segments.next()?;
        match want.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
            Some(name) if !got.is_empty() => params.pairs.push((name, got)),
            Some(_) => return None,
            None if *want == got => {}
            None => return None,
        }
    }
    segments.next().is_none().then_some(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, path: &str) -> Request {
        Request { method: method.into(), path: path.into(), ..Request::default() }
    }

    #[test]
    fn routes_capture_segments_and_refuse_by_404_and_405() {
        let router: Router<u32> = Router::new()
            .get("/", |_, _, _| Response::text(200, "index"))
            .post("/v1/agents/{cid}/tell", |base, _, params| {
                Response::text(200, &format!("{}+{}", params.get("cid"), base))
            });
        let answer = router.handle(&7, &request("POST", "/v1/agents/42/tell"));
        assert_eq!((answer.status, answer.bytes()), (200, &b"42+7"[..]));
        assert_eq!(router.handle(&7, &request("GET", "/")).status, 200);

        let wrong_method = router.handle(&7, &request("GET", "/v1/agents/42/tell"));
        assert_eq!(wrong_method.status, 405);
        assert!(wrong_method.headers.contains(&("Allow".into(), "POST".into())));
        for path in ["/v1/agents//tell", "/v1/agents/42/tell/x", "/v1/agents/42", "/nope"] {
            assert_eq!(router.handle(&7, &request("POST", path)).status, 404, "{path}");
        }
    }
}
