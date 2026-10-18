//! Optional built-in TLS for the TCP listener (cargo feature `tls`).
//!
//! Certificates come from the `[tls]` section of the config (PEM files). The
//! recommended remote setups remain `tailscale serve` or a reverse proxy
//! (architecture 9); this is for people who want neither. Certificates are
//! read once at start-up: restart to pick up a renewal.

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::server::ServerError;

pub async fn serve(
    config: &Config,
    listener: TcpListener,
    router: Router,
    mut stop: watch::Receiver<bool>,
) -> Result<JoinHandle<()>, ServerError> {
    let tls = config.tls.clone().ok_or(ServerError::NoTls)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let std_listener = listener
        .into_std()
        .map_err(|e| ServerError::Io("tls listener".into(), e))?;
    // A bad certificate stops the start-up instead of leaving no listener.
    let cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
        .await
        .map_err(|e| {
            ServerError::Io(
                format!("cannot load certificate {}", tls.cert.display()),
                e,
            )
        })?;
    let handle = axum_server::Handle::new();
    let h2 = handle.clone();
    Ok(tokio::spawn(async move {
        tokio::spawn(async move {
            let _ = stop.wait_for(|s| *s).await;
            h2.graceful_shutdown(Some(std::time::Duration::from_secs(3)));
        });
        match axum_server::from_tcp_rustls(std_listener, cfg) {
            Ok(server) => {
                let _ = server
                    .handle(handle)
                    .serve(router.into_make_service())
                    .await;
            }
            Err(e) => tracing::error!(target: "tls", "cannot start TLS listener: {e}"),
        }
    }))
}
