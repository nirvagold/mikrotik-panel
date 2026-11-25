//! Menyajikan SPA Preact yang ditanam di binary (hasil `npm run build` di web/dist).

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Assets;

const MISSING: &str = "<!doctype html><html lang=\"id\"><meta charset=\"utf-8\"><title>Panel MikroTik</title>\
<body><p>Antarmuka web belum dibangun. Jalankan <code>npm run build</code> di folder web/ lalu build ulang binary.</p></body></html>";

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(file) = (!path.is_empty()).then(|| Assets::get(path)).flatten() {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let mut res = (StatusCode::OK, file.data.into_owned()).into_response();
        let h = res.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap());
        let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        return res;
    }
    // Semua rute lain dirender oleh SPA.
    let html = Assets::get("index.html").map(|f| f.data.into_owned()).unwrap_or_else(|| MISSING.as_bytes().to_vec());
    let mut res = (StatusCode::OK, html).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}
