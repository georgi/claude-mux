//! Starting the server: the data directory, the actor, and both listeners.
//!
//! ```ignore
//! let server = ServerBuilder::new(config).start().await?;
//! // server.app is the AppState; server.tcp_addr is the bound address.
//! server.shutdown().await;
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use cmux_core::engine::ClientAction;
use cmux_core::ids::ClientId;
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::actor::{
    self, ActorHandle, ActorOptions, Clock, CompositeRunner, EffectRunner, PersistRunner,
};
use crate::api::auth::PeerCred;
use crate::api::{self, AppState, Extra, Transport};
use crate::config::{self, Config, ConfigError};
use crate::devices::Devices;
use crate::diag::{Diag, DiagLayer};
use crate::store::{Store, StoreError, StoreWriter};
use crate::term::hub::Hub;

#[derive(Debug)]
pub enum ServerError {
    Config(ConfigError),
    Store(StoreError),
    Io(String, std::io::Error),
    /// Built without the `tls` feature but configured for TLS.
    NoTls,
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::Config(e) => e.fmt(f),
            ServerError::Store(e) => e.fmt(f),
            ServerError::Io(what, e) => write!(f, "{what}: {e}"),
            ServerError::NoTls => f.write_str(
                "config has a [tls] section but this cmuxd was built without the `tls` feature",
            ),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<ConfigError> for ServerError {
    fn from(e: ConfigError) -> Self {
        ServerError::Config(e)
    }
}

impl From<StoreError> for ServerError {
    fn from(e: StoreError) -> Self {
        ServerError::Store(e)
    }
}

type RunnerFactory = Box<dyn FnOnce(ActorHandle) -> Box<dyn EffectRunner> + Send>;

/// Configures and starts a server. The extension points are for later steps:
/// effect runners (PTY and agent spawning) and extra routes (the stream, the
/// agent endpoints).
pub struct ServerBuilder {
    config: Config,
    clock: Clock,
    tick: Option<Duration>,
    runners: Vec<RunnerFactory>,
    hub: Arc<Hub>,
    extra: Option<Box<dyn Fn() -> Extra + Send>>,
    global_tracing: bool,
}

impl ServerBuilder {
    pub fn new(config: Config) -> Self {
        ServerBuilder {
            config,
            clock: Clock::default(),
            tick: Some(Duration::from_secs(1)),
            runners: Vec::new(),
            hub: Arc::new(Hub::new()),
            extra: None,
            global_tracing: false,
        }
    }

    /// The PTY hub the server will use. It exists before the actor does, so
    /// an effect runner given to [`runner`](Self::runner) can capture it:
    /// `let hub = b.hub(); b.runner(move |h| Box::new(MyRunner::new(h, hub)))`.
    pub fn hub(&self) -> Arc<Hub> {
        self.hub.clone()
    }

    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// `None` stops the 1 s ticker (tests that drive time by hand).
    pub fn tick(mut self, every: Option<Duration>) -> Self {
        self.tick = every;
        self
    }

    /// Adds an effect runner, offered effects after the built-in persistence.
    /// It gets the actor handle to report outcomes with `Command::Runner`.
    pub fn runner(
        mut self,
        make: impl FnOnce(ActorHandle) -> Box<dyn EffectRunner> + Send + 'static,
    ) -> Self {
        self.runners.push(Box::new(make));
        self
    }

    /// Extra routes, built once per listener.
    pub fn extra_routes(mut self, make: impl Fn() -> Extra + Send + 'static) -> Self {
        self.extra = Some(Box::new(make));
        self
    }

    /// Send `tracing` events to the diagnostics log (once per process).
    pub fn global_tracing(mut self, on: bool) -> Self {
        self.global_tracing = on;
        self
    }

    pub async fn start(self) -> Result<Server, ServerError> {
        let ServerBuilder {
            config,
            clock,
            tick,
            runners,
            hub,
            extra,
            global_tracing,
        } = self;
        // Fail on bad configuration before touching the disk.
        let tcp_addr = config.tcp_addr()?;
        if config.tls.is_some() && !cfg!(feature = "tls") {
            return Err(ServerError::NoTls);
        }
        let data_dir = config.data_dir();
        let store = Arc::new(Store::open(&data_dir)?);
        let mut state = store.load()?;
        state.data_dir = data_dir.clone();
        // The config is the source of truth for the wait, not the saved copy.
        state.timeouts.permission_wait = config.agents.permission_wait_secs.max(1) * 1000;
        let diag = Diag::open(&data_dir).map_err(|e| ServerError::Io("diagnostics".into(), e))?;
        if global_tracing {
            use tracing_subscriber::layer::SubscriberExt;
            let sub = tracing_subscriber::registry().with(DiagLayer(diag.clone()));
            let _ = tracing::subscriber::set_global_default(sub);
        }
        let writer = StoreWriter::start(store.clone());
        let actor = actor::spawn(
            state,
            ActorOptions {
                clock: clock.clone(),
                tick,
                journal: Some(writer.clone()),
                ..ActorOptions::default()
            },
            move |handle| {
                let mut all: Vec<Box<dyn EffectRunner>> = vec![Box::new(PersistRunner(writer))];
                all.extend(runners.into_iter().map(|make| make(handle.clone())));
                Box::new(CompositeRunner(all))
            },
        );
        let devices = Devices::open(&data_dir, clock.clone())
            .map_err(|e| ServerError::Io("devices.json".into(), e))?;
        let app = AppState::new(actor, devices, config.clone(), diag, data_dir, hub);
        close_orphan_terminals(&app).await;
        let extra = extra.unwrap_or_else(|| Box::new(Extra::default));

        let (stop_tx, stop_rx) = watch::channel(false);
        let mut tasks = Vec::new();

        // The Unix socket.
        let socket_path = config.socket_path();
        let listener = bind_unix(&socket_path)?;
        let router = api::router_with(app.clone(), Transport::Unix, extra());
        tasks.push(tokio::spawn(serve_unix(listener, router, stop_rx.clone())));

        // TCP.
        let mut bound = None;
        if let Some(addr) = tcp_addr {
            let listener = TcpListener::bind(addr)
                .await
                .map_err(|e| ServerError::Io(format!("bind {addr}"), e))?;
            let local = listener
                .local_addr()
                .map_err(|e| ServerError::Io("local_addr".into(), e))?;
            bound = Some(local);
            let router = api::router_with(app.clone(), Transport::Tcp, extra());
            tasks.push(serve_tcp(&config, listener, router, stop_rx.clone()).await?);
        }
        if let Ok(mut b) = app.base_url.write() {
            *b = config.base_url(bound);
        }
        app.diag.log(
            "info",
            "started",
            serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "socket": socket_path,
                "tcp": bound.map(|a| a.to_string()),
            }),
        );
        Ok(Server {
            app,
            tcp_addr: bound,
            socket_path,
            stop: stop_tx,
            tasks,
            _store: store,
        })
    }
}

/// A running server. Dropping it without [`Server::shutdown`] leaves the
/// tasks running until the runtime ends.
pub struct Server {
    pub app: AppState,
    /// The TCP address actually bound (useful with port 0), if TCP is on.
    pub tcp_addr: Option<SocketAddr>,
    pub socket_path: PathBuf,
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    /// Holds the data directory lock.
    _store: Arc<Store>,
}

impl Server {
    /// Stops the listeners, drains the actor, flushes the store and removes
    /// the socket.
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        // Open connections (a stream, say) must not hold the shutdown up.
        for mut t in self.tasks {
            if tokio::time::timeout(Duration::from_secs(3), &mut t)
                .await
                .is_err()
            {
                t.abort();
            }
        }
        self.app.actor.shutdown().await;
        self.app.diag.log("info", "stopped", serde_json::json!({}));
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Loose terminals saved by an earlier run have no PTY any more.
async fn close_orphan_terminals(app: &AppState) {
    let Ok(loose) = app.actor.query(|s| s.loose.clone()).await else {
        return;
    };
    for term in loose {
        if app.hub.get_term(term).is_none() {
            let action = ClientAction::CloseTerminal { term };
            let _ = app.actor.client(ClientId("server".into()), action).await;
        }
    }
}

fn bind_unix(path: &PathBuf) -> Result<UnixListener, ServerError> {
    use std::os::unix::fs::PermissionsExt;
    let io = |what: &str| {
        let what = format!("{what} {}", path.display());
        move |e| ServerError::Io(what, e)
    };
    if let Some(dir) = path.parent() {
        config::ensure_private_dir(dir).map_err(io("create dir for"))?;
    }
    // The data-dir lock is ours, so a socket file here is a stale leftover.
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(io("bind"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io("chmod"))?;
    Ok(listener)
}

async fn wait_stop(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|stop| *stop).await;
}

async fn serve_unix(listener: UnixListener, router: Router, stop: watch::Receiver<bool>) {
    let app = router.into_make_service_with_connect_info::<PeerCred>();
    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(wait_stop(stop))
        .await;
}

async fn serve_tcp(
    config: &Config,
    listener: TcpListener,
    router: Router,
    stop: watch::Receiver<bool>,
) -> Result<JoinHandle<()>, ServerError> {
    if config.tls.is_some() {
        #[cfg(feature = "tls")]
        return crate::tls::serve(config, listener, router, stop).await;
        #[cfg(not(feature = "tls"))]
        return Err(ServerError::NoTls);
    }
    Ok(tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(wait_stop(stop))
            .await;
    }))
}
