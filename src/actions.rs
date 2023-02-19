use std::{
    fs::OpenOptions, io::ErrorKind, net::SocketAddr, path::PathBuf, sync::{Arc, atomic::{AtomicU64, AtomicBool, Ordering::SeqCst}}, time::SystemTime,
};

use axum::{
    extract::{ConnectInfo, Multipart, State},
    http::StatusCode,
    Extension, Form,
};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio_util::codec::{FramedWrite, BytesCodec};
use tracing::{warn, error};
use futures::{stream::StreamExt, SinkExt, TryStreamExt};

use crate::{disksize::Quotas, SharedDirectory};

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

/// Create new file, renaming it if already exists. Also bumps the files quota counter.
fn create_new_file(dir: &::std::path::Path, filename: &str, quotas: &Quotas) -> Result<(std::fs::File, PathBuf), StatusCode> {
    if quotas.files.bump(1) {
        quotas.files.reduce(1);
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mut infix = String::new();
    for i in 1..=20 {
        let path = dir.join(format!("{filename}{infix}"));
        match OpenOptions::new()
            .read(false)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                infix = format!(".{i}");
            }
            Err(e) => {
                quotas.files.reduce(1);
                warn!("Cannot create new file `{path:?}`: {e}");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }
    quotas.files.reduce(1);
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
    Extension(shared_dir): Extension<Arc<SharedDirectory>>,
    State(quotas): State<Arc<Quotas>>,
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

    let body = f.body.into_bytes();
    if quotas.bytes.bump(body.len() as u64) {
        quotas.bytes.reduce(body.len() as u64);
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let (newfile, _) = create_new_file(&shared_dir.dir, &filename, &quotas)?;
    let mut newfile = tokio::fs::File::from_std(newfile);

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
    Extension(shared_dir): Extension<Arc<SharedDirectory>>,
    State(quotas): State<Arc<Quotas>>,
    Form(f): Form<Remove>,
) -> Result<(), StatusCode> {
    println!("{} {} remove {}", easy_ts(), addr, f.filename,);

    if !allowed_filename(&f.filename) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let p = shared_dir.dir.join(f.filename);
    let metadata = std::fs::metadata(&p);
    match std::fs::remove_file(p) {
        Ok(()) => {
            quotas.files.reduce(1);
            match metadata {
                Ok(m) => {
                    quotas.bytes.reduce(m.len());
                },
                Err(e) => {
                    error!("pre-remove metadata was failed: {e}");
                }
            }
        }
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
    Extension(shared_dir): Extension<Arc<SharedDirectory>>,
    State(quotas): State<Arc<Quotas>>,
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

                if quotas.bytes.is_exceed() {
                    return Err((StatusCode::PAYLOAD_TOO_LARGE, "Disk storage quota full"));
                }

                let (file, newname) = match create_new_file(&shared_dir.dir, &filename, &quotas) {
                    Ok(x) => x,
                    Err(code) => {
                        return Err((code, "Failed create a file"))
                    }
                };
                let file = tokio::fs::File::from_std(file);


                let counter = Arc::new(AtomicU64::new(0));
                let counter_ = counter.clone();

                let quota_flag = Arc::new(AtomicBool::new(false));
                let quota_flag_ = quota_flag.clone();

                let quotas_ = quotas.clone();

                let sink = FramedWrite::new(file, BytesCodec::new());
                let sink = <FramedWrite<_, _> as SinkExt<axum::body::Bytes>>::sink_map_err(sink, |e : std::io::Error|anyhow::Error::from(e));

                let stream = field.map_err(|e|anyhow::Error::from(e));
                let stream = stream.map(move |x| {
                    if quota_flag_.load(SeqCst) {
                        anyhow::bail!("Quota exceed");
                    }
                    if let Ok(b) = &x {
                        if quotas_.bytes.bump(b.len() as u64) {
                            quotas_.bytes.reduce(b.len() as u64);
                            quota_flag_.store(true, SeqCst);
                            let remaining = quotas_.bytes.remaining();
                            if remaining > 0 {
                                let b = b.slice(..(remaining as usize));
                                // do not register it in the quota counters - we'll update metadata later
                                return Ok(b); // fast block to fill the quota to the brim (and prevent duplicate uploads)
                            } else {
                                anyhow::bail!("Quota exceed");
                            }
                        }
                        counter_.fetch_add(b.len() as u64, SeqCst);
                    }
                    x
                });

                // Actual data transfer happens here:
                let ret = stream.forward(sink).await;

                if quota_flag.load(SeqCst) {
                    let len_accounted = counter.load(SeqCst);
                    println!("{} {} upload_quota_hit len_so_far={}",  easy_ts(), addr, len_accounted);
                    let mut renamed = newname.clone().into_os_string();
                    renamed.push(".partial");
                    drop(ret);

                    if let Ok(meta) = std::fs::metadata(&newname) {
                        let len_actual = meta.len();
                        if len_actual > len_accounted {
                            quotas.bytes.bump(len_actual - len_accounted);
                        }
                        if len_accounted > len_actual {
                            quotas.bytes.reduce(len_accounted - len_actual);
                        }
                    }

                    let _ = renamore::rename_exclusive(newname, renamed);
                    return Err((StatusCode::PAYLOAD_TOO_LARGE, "Disk storage quota exceed"));
                } else {
                    match ret {
                        Ok(()) => {
                            println!("{} {} upload_finished {:?} len={}", easy_ts(), addr, newname, counter.load(SeqCst));
                        }
                        Err(e) => {
                            warn!("Upload aborted or failed to write file: {e}");
                            return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed upload a file"))
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
