//! Security middleware: response headers, CSRF, and rate limiting.
//!
//! Ported from the middleware in `internal/web/server.go`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::Response;
use rand::RngExt;

use super::error::WebError;
use super::state::AppState;

/// Requests allowed per window, per client.
pub const DEFAULT_RATE_LIMIT: usize = 30;
/// The window those requests are counted over.
pub const DEFAULT_RATE_WINDOW: Duration = Duration::from_secs(60);

/// Cookie carrying the CSRF token.
pub const CSRF_COOKIE: &str = "eruser_csrf";
/// Header HTMX sends the token back in.
pub const CSRF_HEADER: &str = "x-csrf-token";
/// Form field carrying the token on a plain POST.
pub const CSRF_FIELD: &str = "csrf_token";

/// Headers set on every response.
///
/// The policy is stricter than upstream's: eruser serves its own CSS and
/// JavaScript, so nothing needs to be fetched from a CDN. Go's policy
/// allowed cdn.tailwindcss.com, unpkg.com, and Google Fonts, which meant a
/// privacy tool announced every page view to three third parties.
pub async fn security_headers(request: Request, next: Next) -> Response {
    const CSP: &str = "default-src 'self'; \
         script-src 'self' 'unsafe-inline'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; \
         font-src 'self'; \
         connect-src 'self'; \
         frame-ancestors 'none'; \
         form-action 'self'; \
         base-uri 'self'";

    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert("content-security-policy", HeaderValue::from_static(CSP));
    // The UI shows a home address and send history; a cached copy left in a
    // shared browser profile outlives the session.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, must-revalidate"),
    );

    response
}

/// A fixed number of requests per client per window.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    limit: usize,
    window: Duration,
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("limit", &self.limit)
            .field("window", &self.window)
            .finish()
    }
}

impl RateLimiter {
    pub fn new(limit: usize, window: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            limit,
            window,
        }
    }

    /// Whether `key` may make another request now.
    ///
    /// Go swept expired entries from a background goroutine every minute.
    /// Here the sweep happens on the same pass that records the request, so
    /// there is no task to leak and no window where the map holds stale keys.
    pub fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.lock();

        // Opportunistic sweep so a long-lived server does not accumulate one
        // entry per client that ever connected.
        buckets.retain(|_, seen| seen.iter().any(|at| now.duration_since(*at) < self.window));

        let seen = buckets.entry(key.to_string()).or_default();
        seen.retain(|at| now.duration_since(*at) < self.window);

        if seen.len() >= self.limit {
            return false;
        }
        seen.push(now);
        true
    }

    pub fn tracked_keys(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Instant>>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_RATE_LIMIT, DEFAULT_RATE_WINDOW)
    }
}

/// Reject a client making requests too quickly.
pub async fn rate_limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, WebError> {
    // The server binds to localhost, so every request shares one address.
    // The method and path make the key specific enough to stop a runaway
    // poll loop without one page's traffic starving another.
    let key = format!("{} {}", request.method(), request.uri().path());

    if !state.rate_limiter.allow(&key) {
        return Err(WebError::RateLimited);
    }
    Ok(next.run(request).await)
}

/// A CSRF token: random, and compared in constant time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsrfToken(String);

impl CsrfToken {
    pub fn generate() -> Self {
        let bytes: [u8; 32] = rand::rng().random();
        Self(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_string(value: String) -> Self {
        Self(value)
    }

    /// Compare without leaking how much of the token matched.
    pub fn matches(&self, other: &str) -> bool {
        constant_time_eq(self.0.as_bytes(), other.as_bytes())
    }
}

/// Equality that takes the same time whatever the inputs.
///
/// A short-circuiting comparison lets an attacker recover a token one byte at
/// a time by measuring how long the rejection took.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        difference |= x ^ y;
    }
    difference == 0
}

/// Whether a request method changes state and so needs a CSRF check.
pub fn is_state_changing(method: &Method) -> bool {
    !matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS)
}

/// Read a named cookie out of a Cookie header value.
pub fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_string())
    })
}

/// The Set-Cookie value for a token.
///
/// Not `Secure`: the server is plain HTTP on localhost, and a Secure cookie
/// would simply never be sent back. `SameSite=Strict` is what actually
/// carries the protection here, and it is stricter than the Lax mode Go used.
pub fn cookie_header(name: &str, value: &str) -> String {
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Strict")
}

/// Whether an Origin or Referer header points at this server.
///
/// A browser attaches Origin to cross-site form posts, so a request claiming
/// to come from somewhere else is rejected before the token is even checked.
/// A missing header is allowed: same-origin GETs and some clients omit it.
pub fn origin_is_trusted(origin: &str, port: u16) -> bool {
    let Some((_scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, host_port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()),
        None => (authority, None),
    };

    let host_is_local = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
    let port_matches = host_port.is_none_or(|p| p == port);
    host_is_local && port_matches
}

#[cfg(test)]
mod tests;

/// Enforce CSRF protection on state-changing requests.
///
/// Two independent checks, either of which is enough to stop a cross-site
/// post:
///
/// 1. `Origin` (or `Referer`) must name this server, when the browser sends
///    one. Browsers attach `Origin` to cross-site form posts.
/// 2. A random token from an `HttpOnly` cookie must be echoed back, in the
///    `X-CSRF-Token` header for HTMX requests or a `csrf_token` form field
///    for a plain form post. A cross-site page cannot read the cookie, so it
///    cannot produce the token.
///
/// Safe methods skip the check but still get a token minted, so the page
/// that renders a form has one to embed.
pub async fn csrf_protect(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, WebError> {
    let cookies = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let existing = cookie_value(&cookies, CSRF_COOKIE);

    if !is_state_changing(request.method()) {
        // Mint a token for the page being rendered, if it has none yet.
        let token = existing
            .map(CsrfToken::from_string)
            .unwrap_or_else(CsrfToken::generate);

        let mut request = request;
        request.extensions_mut().insert(token.clone());

        let mut response = next.run(request).await;
        set_csrf_cookie(&mut response, &token);
        return Ok(response);
    }

    // A token can only be checked against one that was already issued.
    let Some(expected) = existing.map(CsrfToken::from_string) else {
        return Err(WebError::InvalidCsrf);
    };

    let origin = request
        .headers()
        .get(header::ORIGIN)
        .or_else(|| request.headers().get(header::REFERER))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    // A missing header is allowed — some clients omit it — but a header
    // naming somewhere else is a cross-site request.
    if let Some(origin) = origin
        && !origin_is_trusted(&origin, state.port)
    {
        return Err(WebError::InvalidCsrf);
    }

    let header_token = request
        .headers()
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let (request, presented) = match header_token {
        Some(token) => (request, Some(token)),
        // No header: this is a plain form post, so the token is in the body.
        // The body has to be buffered to look, then put back for the handler.
        None => {
            let (parts, body) = request.into_parts();
            let bytes = axum::body::to_bytes(body, MAX_FORM_BODY)
                .await
                .map_err(|_| WebError::BadRequest("the form was too large to read".into()))?;
            let found = form_field(&bytes, CSRF_FIELD);
            (
                Request::from_parts(parts, axum::body::Body::from(bytes)),
                found,
            )
        }
    };

    match presented {
        Some(token) if expected.matches(&token) => {}
        _ => return Err(WebError::InvalidCsrf),
    }

    let mut response = next.run(request).await;
    set_csrf_cookie(&mut response, &expected);
    Ok(response)
}

/// Cap on a buffered form body. Every form here is a handful of short
/// fields; anything larger is not a form this server serves.
const MAX_FORM_BODY: usize = 64 * 1024;

/// Pull one field out of an `application/x-www-form-urlencoded` body.
fn form_field(body: &[u8], name: &str) -> Option<String> {
    let body = std::str::from_utf8(body).ok()?;
    body.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (percent_decode(key) == name).then(|| percent_decode(value))
    })
}

/// Decode the subset of percent-encoding a form body uses.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or_default();
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// Attach the CSRF cookie without disturbing any others.
///
/// `insert` would replace every Set-Cookie header on the response, which
/// silently dropped the session cookie the setup wizard had just set — the
/// wizard then started a fresh session on every step and lost the answers.
/// `append` adds one more.
fn set_csrf_cookie(response: &mut Response, token: &CsrfToken) {
    if let Ok(value) = HeaderValue::from_str(&cookie_header(CSRF_COOKIE, token.as_str())) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}
