//! The files of the account web, compiled into the program, so that a cloud needs nothing but
//! its binary. Inter is under the SIL Open Font License; its text is served beside the font.

use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

const STYLE: &str = include_str!("assets/style.css");
const SCRIPT: &str = include_str!("assets/passkey.js");
const ICON: &str = include_str!("assets/icon.svg");
const FONT: &[u8] = include_bytes!("assets/inter.woff2");
const FONT_LICENSE: &str = include_str!("assets/inter-OFL.txt");

pub async fn serve(Path(name): Path<String>) -> Response {
    let (content_type, body): (&str, &[u8]) = match name.as_str() {
        "style.css" => ("text/css; charset=utf-8", STYLE.as_bytes()),
        "passkey.js" => ("text/javascript; charset=utf-8", SCRIPT.as_bytes()),
        "icon.svg" => ("image/svg+xml", ICON.as_bytes()),
        "inter.woff2" => ("font/woff2", FONT),
        "inter-OFL.txt" => ("text/plain; charset=utf-8", FONT_LICENSE.as_bytes()),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut response = (StatusCode::OK, body.to_vec()).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}
