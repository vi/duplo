use axum::body;
use axum::body::Full;

use axum::http::HeaderValue;

use axum::body::Empty;

use axum::http::StatusCode;

use axum::body::BoxBody;

use axum::http::Response;

use axum::extract::Path;

use axum;

use axum::http::header;
use axum::http::header::CACHE_CONTROL;
use include_dir::include_dir;

use include_dir::Dir;

static RESOURCES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/res");

#[axum::debug_handler]
pub(crate) async fn serve_embedded(Path(path): Path<String>) -> Response<BoxBody> {
    let mime_type = mime_guess::from_path(&path).first_or_text_plain();

    match RESOURCES.get_file(path) {
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(body::boxed(Empty::new()))
            .unwrap(),
        Some(file) => Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime_type.as_ref()).unwrap(),
            )
            .header(CACHE_CONTROL, "max-age=3600")
            .body(body::boxed(Full::from(file.contents())))
            .unwrap(),
    }
}
