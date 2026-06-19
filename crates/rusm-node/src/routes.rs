//! Declarative HTTP routing — the `[routes]` table from `rusm.toml`.
//!
//! Each entry maps `"METHOD /path/pattern" = "component#action"`: an incoming request is
//! matched to a component + action (with path params extracted), which the serving
//! gateway then dispatches per-request. Pure logic — the matcher is host-tested here;
//! the gateway just calls [`RouteTable::resolve`].
//!
//! Patterns are `/`-separated segments: a literal (`users`), a `:name` parameter
//! (captured), or a trailing `*` wildcard (captures the remainder). Precedence is by
//! specificity — a literal beats a `:param` beats `*` — so `/users/new` wins over
//! `/users/:id`. A path that matches some route but not for this method is a `405`
//! (distinct from an unmatched `404`).

use std::collections::HashMap;

/// One segment of a route pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// A literal segment that must match exactly.
    Literal(String),
    /// A `:name` parameter — matches one segment, captured under `name`.
    Param(String),
    /// A trailing `*` — matches the remaining segments (captured as `*`).
    Wildcard,
}

/// One parsed route: a method + path pattern resolving to `component#action`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Route {
    method: String,
    pattern: Vec<Segment>,
    component: String,
    action: String,
}

/// The outcome of resolving a request against the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A route matched: dispatch to `component#action` with these path params.
    Found {
        component: String,
        action: String,
        params: Vec<(String, String)>,
    },
    /// The path matched a route, but no route for this method (HTTP 405).
    MethodNotAllowed,
    /// No route matched the path (HTTP 404).
    NotFound,
}

/// A compiled set of routes, ready to [`resolve`](Self::resolve) requests.
#[derive(Debug, Clone, Default)]
pub struct RouteTable {
    routes: Vec<Route>,
}

impl RouteTable {
    /// Compile a **per-request HTTP** `[routes]` map (`"METHOD /path" => "component#action"`).
    /// Returns a clear error on a malformed key (no method/path) or value (no `#`).
    pub fn from_map(raw: &HashMap<String, String>) -> Result<RouteTable, String> {
        Self::compile(raw, true)
    }

    /// Compile a **per-connection** (WebSocket/SSE) `[routes]` map: the value is just the
    /// handler **component** (`"GET /events/:plan" => "events"`) — there is no `#action`,
    /// since the component *is* the connection handler. Captured path params still flow to
    /// the handler via its connection context. Otherwise like [`from_map`].
    pub fn from_handler_map(raw: &HashMap<String, String>) -> Result<RouteTable, String> {
        Self::compile(raw, false)
    }

    /// Shared compile for both shapes; `require_action` distinguishes the per-request HTTP
    /// `component#action` value from the per-connection bare-`component` value.
    fn compile(raw: &HashMap<String, String>, require_action: bool) -> Result<RouteTable, String> {
        let mut routes = Vec::with_capacity(raw.len());
        for (key, target) in raw {
            let (method, path) = key
                .split_once(char::is_whitespace)
                .ok_or_else(|| format!("route `{key}` must be `METHOD /path`"))?;
            let method = method.trim().to_ascii_uppercase();
            if method.is_empty() {
                return Err(format!("route `{key}` has no method"));
            }
            let path = path.trim();
            if !path.starts_with('/') {
                return Err(format!("route `{key}` path must start with `/`"));
            }
            let (component, action) = if require_action {
                let (component, action) = target
                    .split_once('#')
                    .ok_or_else(|| format!("route target `{target}` must be `component#action`"))?;
                if component.is_empty() || action.is_empty() {
                    return Err(format!(
                        "route target `{target}` must be `component#action`"
                    ));
                }
                (component.to_string(), action.to_string())
            } else {
                let component = target.trim();
                if component.is_empty() || component.contains('#') {
                    return Err(format!(
                        "route target `{target}` must be a component name \
                         (a per-connection ws/sse handler takes no `#action`)"
                    ));
                }
                (component.to_string(), String::new())
            };
            routes.push(Route {
                method,
                pattern: parse_pattern(path),
                component,
                action,
            });
        }
        Ok(RouteTable { routes })
    }

    /// Whether the table has any routes (an app may use `[[serve]]` without `[routes]`).
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Resolve `(method, target)` (`target` is the request path, with any query string)
    /// to a route. Picks the most specific match (literal > `:param` > `*`); a path that
    /// matches only under a different method is `MethodNotAllowed`.
    pub fn resolve(&self, method: &str, target: &str) -> Resolution {
        let method = method.to_ascii_uppercase();
        let path = target.split(['?', '#']).next().unwrap_or(target);
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        let mut best: Option<(u32, &Route, Vec<(String, String)>)> = None;
        let mut path_matched = false; // some route matched the path (for 405 vs 404)
        for route in &self.routes {
            let Some(params) = match_pattern(&route.pattern, &segments) else {
                continue;
            };
            path_matched = true;
            if route.method != method {
                continue;
            }
            let score = specificity(&route.pattern);
            if best.as_ref().is_none_or(|(top, _, _)| score > *top) {
                best = Some((score, route, params));
            }
        }
        match best {
            Some((_, route, params)) => Resolution::Found {
                component: route.component.clone(),
                action: route.action.clone(),
                params,
            },
            None if path_matched => Resolution::MethodNotAllowed,
            None => Resolution::NotFound,
        }
    }
}

/// Parse a path pattern into segments (`/users/:id` → `[Literal("users"), Param("id")]`).
fn parse_pattern(path: &str) -> Vec<Segment> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if seg == "*" {
                Segment::Wildcard
            } else if let Some(name) = seg.strip_prefix(':') {
                Segment::Param(name.to_string())
            } else {
                Segment::Literal(seg.to_string())
            }
        })
        .collect()
}

/// Match a request's `segments` against a `pattern`, returning the captured params, or
/// `None` if it doesn't match.
fn match_pattern(pattern: &[Segment], segments: &[&str]) -> Option<Vec<(String, String)>> {
    let mut params = Vec::new();
    for (i, seg) in pattern.iter().enumerate() {
        match seg {
            Segment::Literal(lit) => {
                if segments.get(i) != Some(&lit.as_str()) {
                    return None;
                }
            }
            Segment::Param(name) => match segments.get(i) {
                Some(value) => params.push((name.clone(), (*value).to_string())),
                None => return None,
            },
            // A trailing wildcard captures the remainder (one or more segments).
            Segment::Wildcard => {
                let rest = segments.get(i..).unwrap_or(&[]);
                if rest.is_empty() {
                    return None;
                }
                params.push(("*".to_string(), rest.join("/")));
                return Some(params);
            }
        }
    }
    // No wildcard consumed the tail, so the lengths must match exactly.
    (pattern.len() == segments.len()).then_some(params)
}

/// Specificity score for precedence: literal (2) > param (1) > wildcard (0), summed.
fn specificity(pattern: &[Segment]) -> u32 {
    pattern
        .iter()
        .map(|s| match s {
            Segment::Literal(_) => 2,
            Segment::Param(_) => 1,
            Segment::Wildcard => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(pairs: &[(&str, &str)]) -> RouteTable {
        let map = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        RouteTable::from_map(&map).unwrap()
    }

    fn found<'a>(r: &'a Resolution) -> (&'a str, &'a str, &'a [(String, String)]) {
        match r {
            Resolution::Found {
                component,
                action,
                params,
            } => (component, action, params),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn matches_exact_and_extracts_params() {
        let t = table(&[
            ("GET /", "ide#page"),
            ("POST /execute-plan", "commander#execute"),
            ("GET /events/:plan/:collection/:id", "commander#events"),
        ]);
        let r = t.resolve("GET", "/");
        let (c, a, _) = found(&r);
        assert_eq!((c, a), ("ide", "page"));
        let r = t.resolve("POST", "/execute-plan");
        let (c, a, _) = found(&r);
        assert_eq!((c, a), ("commander", "execute"));
        let r = t.resolve("GET", "/events/p1/pages/x?last=3"); // query stripped
        let (c, a, params) = found(&r);
        assert_eq!((c, a), ("commander", "events"));
        assert_eq!(
            params,
            &[
                ("plan".into(), "p1".into()),
                ("collection".into(), "pages".into()),
                ("id".into(), "x".into()),
            ]
        );
    }

    #[test]
    fn per_connection_routes_take_a_bare_component_and_capture_params() {
        // ws/sse routes name the handler component only (no `#action`) — the component is
        // the connection handler — but path params are still captured for its context.
        let map = [
            ("GET /events/:plan/:collection/:id", "events"),
            ("GET /stream/:app", "stream"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let t = RouteTable::from_handler_map(&map).unwrap();

        let r = t.resolve("GET", "/events/p7/pages/42");
        let (component, action, params) = found(&r);
        assert_eq!(component, "events");
        assert_eq!(action, "", "per-connection routes have no action");
        assert_eq!(
            params,
            &[
                ("plan".into(), "p7".into()),
                ("collection".into(), "pages".into()),
                ("id".into(), "42".into()),
            ]
        );
        let stream = t.resolve("GET", "/stream/app-1");
        let (component, _, params) = found(&stream);
        assert_eq!(component, "stream");
        assert_eq!(params, &[("app".into(), "app-1".into())]);
    }

    #[test]
    fn per_connection_route_rejects_an_action() {
        // A `#action` in a per-connection route value is a mistake (no action concept).
        let map = [("GET /stream/:app", "stream#open")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let err = RouteTable::from_handler_map(&map).unwrap_err();
        assert!(err.contains("no `#action`"), "clear error: {err}");
    }

    #[test]
    fn literal_beats_param_beats_wildcard() {
        let t = table(&[
            ("GET /users/:id", "users#show"),
            ("GET /users/new", "users#new"),
            ("GET /users/*", "users#catchall"),
        ]);
        assert_eq!(found(&t.resolve("GET", "/users/new")).1, "new"); // literal wins
        assert_eq!(found(&t.resolve("GET", "/users/42")).1, "show"); // param over wildcard
        let (a, params) = {
            let r = t.resolve("GET", "/users/42/posts");
            let (_, a, p) = found(&r);
            (a.to_string(), p.to_vec())
        };
        assert_eq!(a, "catchall"); // only the wildcard spans 2 trailing segments
        assert_eq!(params, vec![("*".to_string(), "42/posts".to_string())]);
    }

    #[test]
    fn method_mismatch_is_405_unknown_path_is_404() {
        let t = table(&[
            ("POST /users", "users#create"),
            ("GET /users/:id", "users#show"),
        ]);
        assert_eq!(t.resolve("GET", "/users"), Resolution::MethodNotAllowed);
        assert_eq!(
            t.resolve("DELETE", "/users/1"),
            Resolution::MethodNotAllowed
        );
        assert_eq!(t.resolve("GET", "/nope"), Resolution::NotFound);
    }

    #[test]
    fn rejects_malformed_entries() {
        let bad = |k: &str, v: &str| {
            RouteTable::from_map(&HashMap::from([(k.to_string(), v.to_string())])).is_err()
        };
        assert!(bad("GET", "users#show")); // no path (no whitespace)
        assert!(bad("GET users", "users#show")); // path missing leading /
        assert!(bad("GET /users", "usersshow")); // target missing #
        assert!(bad("GET /users", "#show")); // empty component
        assert!(RouteTable::from_map(&HashMap::new()).unwrap().is_empty()); // empty is fine
    }
}
