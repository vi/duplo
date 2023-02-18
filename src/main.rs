use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    http::StatusCode,
    response::Redirect,
    routing::{get, get_service, post},
    Extension, Router,
};
use tower_http::services::ServeDir;

/// simple insecure HTTP server with anonymous file upload (including html/js upload and publication)
#[derive(argh::FromArgs)]
struct Opts {
    /// socket address to bind TCP socket and listen for including HTTP requests
    #[argh(positional)]
    listen_socket: SocketAddr,

    /// serve (and upload) files from this directory at /transient/
    #[argh(positional)]
    transiet_directory: PathBuf,

    /// serve (and upload) files from this directory at /permanent/
    #[argh(positional)]
    permanent_directory: PathBuf,
}

mod actions;
mod embedded_resources;
mod file_list;

async fn handle_error(_err: std::io::Error) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let opts: Opts = argh::from_env();
    tracing_subscriber::fmt::init();

    let listen_socket = opts.listen_socket;

    let app = Router::new()
        .route("/", get(file_list::serve_view))
        .route("/shareText/", post(actions::share_text))
        .route("/remove/", post(actions::remove))
        .route("/upload/", post(actions::upload));
    let app_transient = app
        .clone()
        .fallback_service(
            get_service(ServeDir::new(opts.transiet_directory.clone())).handle_error(handle_error),
        )
        .layer(Extension(Arc::new(opts.transiet_directory)));
    let app_permanent = app
        .fallback_service(
            get_service(ServeDir::new(opts.permanent_directory.clone())).handle_error(handle_error),
        )
        .layer(Extension(Arc::new(opts.permanent_directory)));

    let routes = Router::new()
        .route("/", get(|| async { Redirect::permanent("/transient/") }))
        .nest("/transient", app_transient)
        .nest("/permanent", app_permanent)
        .route("/persistent/", get(file_list::serve_view))
        .route("/res/*path", get(embedded_resources::serve_embedded))
        //.with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http());

    axum::Server::try_bind(&listen_socket)?
        .serve(routes.into_make_service_with_connect_info::<SocketAddr>())
        .await?;
    Ok(())
}
