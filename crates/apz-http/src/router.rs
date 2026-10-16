//! Method + path-pattern dispatch.

use std::borrow::Cow;

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

/// The segments a pattern's `{name}`s captured, percent-decoded.
#[derive(Debug, Default)]
pub struct Params<'a> {
    pairs: Vec<(&'static str, Cow<'a, str>)>,
}

impl Params<'_> {
    /// The segment captured as `{name}`, decoded (`%40` is `@`, `%2F` a slash inside the value); empty when
    /// the pattern has none.
    pub fn get(&self, name: &str) -> &str {
        self.pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map_or("", |(_, value)| value.as_ref())
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
            let params = match matches(&route.pattern, &request.path) {
                None => continue,
                Some(Err(())) => {
                    return Response::error(
                        400,
                        "a path parameter is not valid UTF-8 once percent-decoded",
                    );
                }
                Some(Ok(params)) => params,
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
        Response::error(405, &format!("this endpoint accepts {allowed}"))
            .with_header("Allow", &allowed)
    }
}

/// Percent-decodes one raw path segment. A malformed escape stays as it is; `None` when the decoded bytes
/// are not UTF-8.
fn decode_segment(raw: &str) -> Option<Cow<'_, str>> {
    if !raw.contains('%') {
        return Some(Cow::Borrowed(raw));
    }
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) =
                std::str::from_utf8(&bytes[i + 1..i + 3]).map(|hex| u8::from_str_radix(hex, 16))
            && let Ok(byte) = byte
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok().map(Cow::Owned)
}

/// `None` when the pattern does not match; `Some(Err(()))` when it matches but a captured value is not
/// valid UTF-8 once decoded (answered with 400).
fn matches<'a>(pattern: &[&'static str], path: &'a str) -> Option<Result<Params<'a>, ()>> {
    let body = path.strip_prefix('/')?;
    let mut segments = body.split('/');
    let mut pairs: Vec<(&'static str, Cow<'a, str>)> = Vec::new();
    let mut offset = 0;
    for want in pattern {
        // `{name..}` as the last segment captures the rest of the path (one or more segments). Each
        // segment is decoded on its own, so an escaped slash inside one stays in that segment's value.
        if let Some(name) = want
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix("..}"))
        {
            let rest = body.get(offset..).filter(|rest| !rest.is_empty())?;
            let Some(parts) = rest
                .split('/')
                .map(decode_segment)
                .collect::<Option<Vec<_>>>()
            else {
                return Some(Err(()));
            };
            pairs.push((name, Cow::Owned(parts.join("/"))));
            return Some(Ok(Params { pairs }));
        }
        let got = segments.next()?;
        offset += got.len() + 1;
        match want
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
        {
            Some(name) if !got.is_empty() => match decode_segment(got) {
                Some(value) => pairs.push((name, value)),
                None => return Some(Err(())),
            },
            Some(_) => return None,
            None if *want == got => {}
            None => return None,
        }
    }
    segments.next().is_none().then_some(Ok(Params { pairs }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, path: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            ..Request::default()
        }
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
        assert!(
            wrong_method
                .headers
                .contains(&("Allow".into(), "POST".into()))
        );
        for path in [
            "/v1/agents//tell",
            "/v1/agents/42/tell/x",
            "/v1/agents/42",
            "/nope",
        ] {
            assert_eq!(
                router.handle(&7, &request("POST", path)).status,
                404,
                "{path}"
            );
        }
    }

    #[test]
    fn a_wildcard_segment_captures_the_rest_of_the_path() {
        let router: Router<u32> = Router::new()
            .get("/v1/hf/models/{id..}", |_, _, params| {
                Response::text(200, params.get("id"))
            })
            .get("/v1/hf/models", |_, _, _| Response::text(200, "list"));
        let answer = router.handle(&0, &request("GET", "/v1/hf/models/org/name"));
        assert_eq!((answer.status, answer.bytes()), (200, &b"org/name"[..]));
        let single = router.handle(&0, &request("GET", "/v1/hf/models/gpt2"));
        assert_eq!((single.status, single.bytes()), (200, &b"gpt2"[..]));
        assert_eq!(
            router.handle(&0, &request("GET", "/v1/hf/models")).bytes(),
            b"list"
        );
        assert_eq!(
            router.handle(&0, &request("GET", "/v1/hf/models/")).status,
            404
        );
    }

    #[test]
    fn path_captures_decode_and_keep_an_escaped_slash_in_its_segment() {
        // Captured values are percent-decoded, and an escaped slash stays inside its own segment.
        {
            let router: Router<u32> = Router::new()
                .get("/v1/recipes/{id}", |_, _, params| {
                    Response::text(200, params.get("id"))
                })
                .get("/v1/hf/models/{id..}", |_, _, params| {
                    Response::text(200, params.get("id"))
                });
            let get = |path: &str| router.handle(&0, &request("GET", path));
            // A plain segment is unchanged.
            assert_eq!(get("/v1/recipes/plain").bytes(), b"plain");
            // `%40` is `@`: the recipe name the Lab sends as `rung-predictor%401`.
            assert_eq!(
                get("/v1/recipes/rung-predictor%401").bytes(),
                b"rung-predictor@1"
            );
            // `%2F` inside a single segment is part of that one value, not a path separator.
            let single = get("/v1/recipes/a%2Fb");
            assert_eq!((single.status, single.bytes()), (200, &b"a/b"[..]));
            // In a `{name..}` capture an escaped slash decodes to a slash, and each segment decodes on its own.
            assert_eq!(get("/v1/hf/models/org%2Fname").bytes(), b"org/name");
            assert_eq!(get("/v1/hf/models/org/a%20b").bytes(), b"org/a b");
            assert_eq!(get("/v1/hf/models/org/plain").bytes(), b"org/plain");
            // A malformed escape stays as it is; invalid UTF-8 is a 400.
            assert_eq!(get("/v1/recipes/100%").bytes(), b"100%");
            assert_eq!(get("/v1/recipes/%FF").status, 400);
            assert_eq!(get("/v1/hf/models/org/%FF").status, 400);
        }
        // The web sends the provider/model key encoded, so it is one segment whose value holds the slash.
        {
            let router: Router<u32> = Router::new().get("/v1/prices/{model}", |_, _, params| {
                Response::text(200, params.get("model"))
            });
            let encoded = router.handle(
                &0,
                &request("GET", "/v1/prices/anthropic%2Fclaude-opus-5-5"),
            );
            assert_eq!(
                (encoded.status, encoded.bytes()),
                (200, &b"anthropic/claude-opus-5-5"[..])
            );
            // Unescaped, the slash is a path separator and the one-segment pattern no longer matches.
            let split = router.handle(&0, &request("GET", "/v1/prices/anthropic/claude-opus-5-5"));
            assert_eq!(split.status, 404);
        }
    }
}
