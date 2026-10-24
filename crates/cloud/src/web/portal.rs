//! Serves the web portal: a single-page app, built to static files, from the cloud's own origin.
//!
//! `portal_dir` names the folder of the build. Its `index.html` answers the routes of the app
//! (`/`, `/sign-in`, `/account/...`), and its other files are served as they are: the hashed
//! files in `assets/` for a year, the rest for an hour. Nothing here shadows the routes of the
//! cloud: those are more specific, and a path that is neither a route of the app nor a file
//! answers with the cloud's `not_found`.
//!
//! The answers carry the security headers of the account web ([`super::guard`]); the policy
//! allows only the cloud's own scripts and styles.
//!
//! `/u/{name}` is a public profile. The shell for it carries the profile's title and
//! description as Open Graph tags, so a link to it previews. The tags hold only what a signed-out
//! visitor may see.

use std::path::{Component, Path as FsPath, PathBuf};

use anyplay_api::Part;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::api::{limit_public, owner_of};
use super::guard;
use crate::error::AppError;
use crate::http::ClientIp;
use crate::state::AppState;
use crate::visibility::viewing_as;

/// The routes of the app that `index.html` answers. A route ending in `/{*rest}` also answers
/// its deeper paths.
const APP_ROUTES: &[&str] = &[
    "/",
    "/sign-in",
    "/create",
    "/account",
    "/account/{*rest}",
    "/friends",
    "/friends/{*rest}",
    "/trophies",
    "/trophies/{*rest}",
];

/// The routes of the app, its files, and the headers. Merge it where the account web is.
pub fn router(state: &AppState) -> Router<AppState> {
    let mut router = Router::new();
    for route in APP_ROUTES {
        router = router.route(route, get(shell));
    }
    router
        .route("/u/{name}", get(profile_shell))
        .route("/assets/{*path}", get(asset))
        .route("/{file}", get(root_file))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
}

fn portal_dir(state: &AppState) -> Result<&FsPath, AppError> {
    state
        .config
        .portal_dir
        .as_deref()
        .ok_or_else(AppError::not_found)
}

/// `dir` joined with `relative`, when `relative` stays inside it.
fn inside(dir: &FsPath, relative: &str) -> Option<PathBuf> {
    // A backslash or a colon is a separator or a drive on some systems: never part of a name here.
    if relative.contains(['\\', ':', '\0']) {
        return None;
    }
    let relative = FsPath::new(relative);
    let plain = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    (plain && relative.components().next().is_some()).then(|| dir.join(relative))
}

fn content_type(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn file_response(path: &FsPath, bytes: Vec<u8>, cache: &'static str) -> Response {
    let mut response = (StatusCode::OK, bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// A file of the build, or `not_found`.
async fn serve_file(state: &AppState, relative: &str, cache: &'static str) -> Response {
    let found = async {
        let dir = portal_dir(state)?;
        let path = inside(dir, relative).ok_or_else(AppError::not_found)?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| AppError::not_found())?;
        Ok::<_, AppError>(file_response(&path, bytes, cache))
    };
    match found.await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn index(state: &AppState) -> Result<String, AppError> {
    let path = portal_dir(state)?.join("index.html");
    let bytes = tokio::fs::read(&path).await.map_err(|error| {
        tracing::error!(%error, "cannot read the portal's index.html");
        AppError::internal()
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn html(status: StatusCode, body: String) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    // The shell is small and names the hashed files of this build: a browser asks again.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// `index.html`, for every route of the app.
async fn shell(State(state): State<AppState>) -> Response {
    match index(&state).await {
        Ok(body) => html(StatusCode::OK, body),
        Err(error) => error.into_response(),
    }
}

/// A file in `assets/`. The build names these by the hash of their content.
async fn asset(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    serve_file(
        &state,
        &format!("assets/{path}"),
        "public, max-age=31536000, immutable",
    )
    .await
}

/// A file at the top of the build, such as the icon.
async fn root_file(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    if file == "index.html" {
        return shell(State(state)).await;
    }
    serve_file(&state, &file, "public, max-age=3600").await
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Puts the tags before `</head>`, and the title in place of the shell's.
fn with_tags(shell: &str, title: &str, tags: &str) -> String {
    let lower = shell.to_ascii_lowercase();
    let mut out = shell.to_owned();
    // The title first: it sits before `</head>`, so the later offset is still right.
    if let (Some(open), Some(close)) = (lower.find("<title>"), lower.find("</title>"))
        && open + 7 <= close
    {
        out.replace_range(open + 7..close, &escape(title));
    }
    let lower = out.to_ascii_lowercase();
    match lower.find("</head>") {
        Some(at) => out.insert_str(at, tags),
        None => out.insert_str(0, tags),
    }
    out
}

/// `index.html` for `/u/{name}`, with the profile's title and description. The page of a player
/// who does not exist answers 404 with the plain shell, so the app shows its own message and a
/// search engine drops the address.
async fn profile_shell(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(name): Path<String>,
) -> Response {
    let built = async {
        limit_public(&state, ip)?;
        let page = index(&state).await?;
        let Ok(owner) = owner_of(&state, &name).await else {
            return Ok(html(StatusCode::NOT_FOUND, page));
        };
        // What a signed-out visitor sees, whoever asks: the tags go to link previews.
        let seen = viewing_as(&state.db, None, owner.id()).await?;
        let about = if seen.can(Part::About) {
            crate::db::profiles::profile_of(&state.db, owner.id())
                .await?
                .0
        } else {
            String::new()
        };
        let site = &state.config.name;
        let title = format!("{} on {site}", owner.handle);
        let description = if about.is_empty() {
            format!("{} plays on {site}.", owner.handle)
        } else {
            about.chars().take(200).collect()
        };
        let url = state
            .web
            .as_ref()
            .map(|web| format!("{}/u/{}", web.origin, owner.handle))
            .unwrap_or_default();
        let tags = format!(
            "<meta name=\"description\" content=\"{d}\">\
             <meta property=\"og:site_name\" content=\"{s}\">\
             <meta property=\"og:type\" content=\"profile\">\
             <meta property=\"og:title\" content=\"{t}\">\
             <meta property=\"og:description\" content=\"{d}\">\
             <meta property=\"og:url\" content=\"{u}\">\
             <meta name=\"twitter:card\" content=\"summary\">",
            d = escape(&description),
            s = escape(site),
            t = escape(&title),
            u = escape(&url),
        );
        Ok::<_, AppError>(html(StatusCode::OK, with_tags(&page, &title, &tags)))
    };
    match built.await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_build() {
        let dir = FsPath::new("/portal");
        assert!(inside(dir, "assets/app-1a2b.js").is_some());
        assert!(inside(dir, "favicon.svg").is_some());
        for bad in [
            "",
            "..",
            "../etc/passwd",
            "assets/../../x",
            "/etc/passwd",
            "a\\b",
            "c:x",
        ] {
            assert!(inside(dir, bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn tags_go_before_the_end_of_the_head_and_the_title_is_replaced() {
        let shell = "<!doctype html><html><head><TITLE>AnyPlay</TITLE></head><body></body></html>";
        let out = with_tags(shell, "a <b> & c", "<meta x>");
        assert_eq!(
            out,
            "<!doctype html><html><head><TITLE>a &lt;b&gt; &amp; c</TITLE><meta x></head><body></body></html>"
        );
    }

    #[test]
    fn text_is_escaped() {
        assert_eq!(
            escape("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }
}
