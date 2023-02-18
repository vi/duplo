use std::{
    fs::OpenOptions, io::ErrorKind, net::SocketAddr, path::PathBuf, sync::{Arc, atomic::AtomicU64}, time::SystemTime,
};

use axum::{
    extract::{ConnectInfo, Multipart},
    http::StatusCode,
    Extension, Form,
};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio_util::codec::{FramedWrite, BytesCodec};
use tracing::warn;
use futures::{stream::StreamExt, SinkExt, TryStreamExt};

fn allowed_filename(x: &str) -> bool {
    if x.contains("..") {
        return false;
    }
    if x.contains('/') {
        return false;
    }
    true
}

fn easy_ts() -> impl std::fmt::Display {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|x| x.as_secs())
        .unwrap_or(0)
}

/// Create new file, renaming it if already exists
fn create_new_file(dir: &::std::path::Path, filename: &str) -> Result<std::fs::File, StatusCode> {
    let mut infix = String::new();
    for i in 1..=20 {
        let path = dir.join(format!("{filename}{infix}"));
        match OpenOptions::new()
            .read(false)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok(f),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                infix = format!(".{i}");
            }
            Err(e) => {
                warn!("Cannot create new file `{path:?}`: {e}");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }
    Err(StatusCode::CONFLICT)
}

#[derive(Deserialize)]
pub(crate) struct ShareText {
    title: String,
    body: String,
}

#[axum::debug_handler]
pub(crate) async fn share_text(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(dir): Extension<Arc<PathBuf>>,
    Form(f): Form<ShareText>,
) -> Result<(), StatusCode> {
    println!(
        "{} {} share_text {} len={}",
        easy_ts(),
        addr,
        f.title,
        f.body.len()
    );

    let mut filename = f.title;

    if !allowed_filename(&filename) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !filename.ends_with(".txt") {
        filename += ".txt";
    }

    let newfile = create_new_file(&dir, &filename)?;
    let mut newfile = tokio::fs::File::from_std(newfile);
    let body = f.body.into_bytes();

    match newfile.write_all(&body).await {
        Ok(()) => (),
        Err(e) => {
            warn!("share_text: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct Remove {
    #[serde(rename = "fileName")]
    filename: String,
}

#[axum::debug_handler]
pub(crate) async fn remove(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(dir): Extension<Arc<PathBuf>>,
    Form(f): Form<Remove>,
) -> Result<(), StatusCode> {
    println!("{} {} remove {}", easy_ts(), addr, f.filename,);

    if !allowed_filename(&f.filename) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let p = dir.join(f.filename);
    match std::fs::remove_file(p) {
        Ok(()) => (),
        Err(e) => {
            warn!("remove: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    Ok(())
}

#[axum::debug_handler]
pub(crate) async fn upload(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(dir): Extension<Arc<PathBuf>>,
    mut multipart: Multipart,
) -> Result<(), (StatusCode, &'static str)> {
    loop {
        match multipart.next_field().await {
            Err(_e) => return Err((StatusCode::BAD_REQUEST, "Failed to read multipart")),
            Ok(None) => break,
            Ok(Some(field)) => {
                let Some(filename) = field.file_name() else { continue };
                let filename =filename.to_owned();

                println!("{} {} upload {}", easy_ts(), addr, filename);

                if !allowed_filename(&filename) {
                    return Err((StatusCode::BAD_REQUEST, "This filename is not allowed"));
                }

                let file = match create_new_file(&dir, &filename) {
                    Ok(x) => x,
                    Err(code) => {
                        return Err((code, "Failed create a file"))
                    }
                };
                let file = tokio::fs::File::from_std(file);


                let counter = Arc::new(AtomicU64::new(0));
                let counter_ = counter.clone();

                let sink = FramedWrite::new(file, BytesCodec::new());
                let sink = <FramedWrite<_, _> as SinkExt<axum::body::Bytes>>::sink_map_err(sink, |e : std::io::Error|anyhow::Error::from(e));

                let stream = field.map_err(|e|anyhow::Error::from(e));
                let stream = stream.inspect(move |x| {
                    if let Ok(b) = x {
                        counter_.fetch_add(b.len() as u64, std::sync::atomic::Ordering::SeqCst);
                    }
                });
                match stream.forward(sink).await {
                    Ok(()) => {
                        println!("{} {} upload_finished {} len={}", easy_ts(), addr, filename, counter.load(std::sync::atomic::Ordering::SeqCst));
                    }
                    Err(e) => {
                        warn!("Upload aborted or failed to write file: {e}");
                        return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed upload a file"))
                    }
                }
            }
        }
    }

    Ok(())
}
