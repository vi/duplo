use std::{sync::Arc, time::UNIX_EPOCH, path::PathBuf};

use askama::Template;
use axum::{self, http::{StatusCode, HeaderValue}, Extension};
use axum::http::header::CACHE_CONTROL;
use axum::response::Response;
use humansize::BINARY;
use askama_axum::IntoResponse;


pub struct FileInfo {
    pub time: u64,
    pub name: String,
    pub size: String,
}

#[derive(Template)]
#[template(path = "view.html")]
pub struct ViewTemplate {
    pub title: String,
    pub files: Vec<FileInfo>,
    pub err: String,
}

#[axum::debug_handler]
pub(crate) async fn serve_view(
    Extension(dir): Extension<Arc<PathBuf>>,
) -> Result<Response, StatusCode> {
    let files = std::fs::read_dir(&*dir).map_err(|e| {
        tracing::error!("readdir: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut err = String::new();
    let files = files.flat_map(|f| {
        match f {
            Err(e) => {
                tracing::error!("readdir 2: {e}");
                err = format!("{e}");
                None
            }
            Ok(f) => {
                if let Ok(name) = f.file_name().into_string() {
                    if name.starts_with('.') { return None; }
                    let mut time = 0;
                    let mut size = String::new();
                    if let Ok(metadata) = f.metadata() {
                        size = humansize::format_size(metadata.len(), BINARY);
                        if let Ok(modified) = metadata.modified() {
                            if let Ok(dur) = modified.duration_since(UNIX_EPOCH) {
                                time = dur.as_secs();
                            }
                        }
                    }
                    Some(FileInfo {
                        name,
                        size,
                        time,
                    })
                } else {
                    err = "Malformed filename skipped from the list".to_owned();
                    None
                }
            }
        }
    }).collect();
    let mut response = ViewTemplate {
        title: "Duplo".to_owned(),
        files,
        err,
    }.into_response();
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}
