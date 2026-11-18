use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../avrig-ai/apps/web/dist"]
pub struct WebAssets;

pub async fn static_asset_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // If empty or path leads to root, serve index.html
    let target_path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = WebAssets::get(target_path) {
        return build_asset_response(target_path, &file.data);
    }

    // If the path looks like a direct asset/file request (has a dot extension or starts with assets/), return 404
    if target_path.starts_with("assets/") || target_path.contains('.') {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    // SPA fallback: serve index.html for client-side routing
    if let Some(index_file) = WebAssets::get("index.html") {
        build_asset_response("index.html", &index_file.data)
    } else {
        (StatusCode::NOT_FOUND, "index.html not found in embedded web assets").into_response()
    }
}

fn build_asset_response(path: &str, data: &[u8]) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut headers = HeaderMap::new();

    headers.insert(
        header::CONTENT_TYPE,
        mime.as_ref()
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );

    let cache_control = if path.starts_with("assets/") {
        // Vite hashed assets are immutable and safe to cache for a long time
        "public, max-age=31536000, immutable"
    } else {
        // HTML and non-hashed files should revalidate
        "no-cache"
    };

    headers.insert(
        header::CACHE_CONTROL,
        cache_control.parse().unwrap(),
    );

    (headers, Body::from(data.to_vec())).into_response()
}
