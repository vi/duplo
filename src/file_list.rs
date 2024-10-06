use std::{sync::Arc, time::UNIX_EPOCH};

use askama::Template;
use askama_axum::IntoResponse;
use axum::extract::OriginalUri;
use axum::http::header::CACHE_CONTROL;
use axum::response::{Redirect, Response};
use axum::{
    self,
    extract::State,
    http::{HeaderValue, StatusCode},
    Extension,
};
use humansize::BINARY;

use crate::disksize::Quotas;
use crate::SharedDirectory;

pub struct FileInfo {
    pub time: u64,
    pub name: String,
    pub size: String,

    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
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
    OriginalUri(uri): OriginalUri,
    Extension(shared_dir): Extension<Arc<SharedDirectory>>,
    State(quotas): State<Arc<Quotas>>,
) -> Result<Response, StatusCode> {
    if !uri.path().ends_with('/') {
        return Ok(Redirect::permanent(&format!("{}/", uri.path())).into_response());
    }
    let files = std::fs::read_dir(&*shared_dir.dir).map_err(|e| {
        tracing::error!("readdir: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut err = String::new();
    if quotas.files.is_exceed() {
        err += "Too many files\n"
    }
    if quotas.bytes.is_close_to_exeeed() {
        if quotas.bytes.is_exceed() {
            err += "Disk storage quota full\n"
        } else {
            err += "Disk storage quota is close to being full\n"
        }
    }
    let mut files: Vec<FileInfo> = files
        .flat_map(|f| match f {
            Err(e) => {
                tracing::error!("readdir 2: {e}");
                err += &format!("{e}");
                None
            }
            Ok(f) => {
                if let Ok(mut name) = f.file_name().into_string() {
                    if name.starts_with('.') {
                        return None;
                    }
                    let mut time = 0;
                    let mut size = String::new();
                    if let Ok(metadata) = f.metadata() {
                        size = humansize::format_size(metadata.len(), BINARY);
                        if let Ok(modified) = metadata.modified() {
                            if let Ok(dur) = modified.duration_since(UNIX_EPOCH) {
                                time = dur.as_secs();
                            }
                        }
                        if metadata.is_dir() {
                            name += "/";
                        }
                    }
                    let tf = time::OffsetDateTime::from_unix_timestamp(time as i64)
                        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
                    Some(FileInfo {
                        name,
                        size,
                        time,
                        year: tf.year(),
                        month: tf.month().into(),
                        day: tf.day(),
                        hour: tf.hour(),
                        minute: tf.minute(),
                        second: tf.second(),
                    })
                } else {
                    err += "Malformed filename skipped from the list";
                    None
                }
            }
        })
        .collect();
    files.sort_by_key(|fi| fi.time);
    let mut response = ViewTemplate {
        title: shared_dir.title.clone(),
        files,
        err,
    }
    .into_response();
    let h = response.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(axum::http::header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; img-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; font-src 'self'; frame-ancestors 'none'"));
    Ok(response)
}
