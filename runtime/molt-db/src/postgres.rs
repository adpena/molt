//! Async Postgres connector for Molt DB integrations.

use crate::{AsyncAcquireError, AsyncPool, AsyncPooled, CancelToken};
use rustls::{ClientConfig, RootCertStore};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_postgres::config::SslMode;
use tokio_postgres::{Client, Config, Error as PgError, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Clone, Debug)]
pub struct PgPoolConfig {
    pub dsn: String,
    pub min_conns: usize,
    pub max_conns: usize,
    pub max_idle: Option<Duration>,
    pub connect_timeout: Duration,
    pub query_timeout: Duration,
    pub max_wait: Duration,
    pub health_check_interval: Option<Duration>,
    pub statement_cache_size: usize,
    pub ssl_root_cert: Option<std::path::PathBuf>,
}

impl PgPoolConfig {
    pub fn new(dsn: String) -> Self {
        Self {
            dsn,
            min_conns: 0,
            max_conns: 16,
            max_idle: None,
            connect_timeout: Duration::from_secs(5),
            query_timeout: Duration::from_secs(2),
            max_wait: Duration::from_millis(250),
            health_check_interval: None,
            statement_cache_size: 128,
            ssl_root_cert: None,
        }
    }
}

pub struct PgConn {
    client: Client,
    cancel_token: tokio_postgres::CancelToken,
    tls: PgTls,
    last_used: Mutex<Instant>,
}

#[derive(Clone)]
enum PgTls {
    None,
    Rustls(MakeRustlsConnect),
}

impl PgConn {
    async fn connect(config: &PgPoolConfig) -> Result<Self, String> {
        let mut pg_config = Config::from_str(&config.dsn)
            .map_err(|err| format!("invalid Postgres DSN: {err}"))?;
        pg_config.connect_timeout(config.connect_timeout);
        let ssl_mode = pg_config.get_ssl_mode();
        let (client, tls) = if ssl_mode == SslMode::Disable {
            let (client, connection) = pg_config
                .connect(NoTls)
                .await
                .map_err(|err| err.to_string())?;
            tokio::spawn(async move {
                if let Err(err) = connection.await {
                    eprintln!("Postgres connection error: {err}");
                }
            });
            (client, PgTls::None)
        } else {
            let tls = build_tls_connector(config)?;
            let (client, connection) = pg_config
                .connect(tls.clone())
                .await
                .map_err(|err| err.to_string())?;
            tokio::spawn(async move {
                if let Err(err) = connection.await {
                    eprintln!("Postgres connection error: {err}");
                }
            });
            (client, PgTls::Rustls(tls))
        };
        let cancel_token = client.cancel_token();
        Ok(Self {
            client,
            cancel_token,
            tls,
            last_used: Mutex::new(Instant::now()),
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn cancel_token(&self) -> &tokio_postgres::CancelToken {
        &self.cancel_token
    }

    pub async fn cancel_query(&self) -> Result<(), PgError> {
        match &self.tls {
            PgTls::None => self.cancel_token.cancel_query(NoTls).await,
            PgTls::Rustls(tls) => self.cancel_token.cancel_query(tls.clone()).await,
        }
    }

    pub fn idle_for(&self) -> Duration {
        let guard = self.last_used.lock().unwrap();
        guard.elapsed()
    }

    pub fn touch(&self) {
        let mut guard = self.last_used.lock().unwrap();
        *guard = Instant::now();
    }

    pub async fn ping(&self) -> Result<(), PgError> {
        self.client.simple_query("SELECT 1").await.map(|_| ())
    }
}

pub struct PgPool {
    config: Arc<PgPoolConfig>,
    pool: Arc<AsyncPool<PgConn>>,
}

impl PgPool {
    pub async fn new(config: PgPoolConfig) -> Result<Self, String> {
        let config = Arc::new(config);
        let pool = {
            let cfg = config.clone();
            AsyncPool::new(config.max_conns, move || {
                let cfg = cfg.clone();
                async move { PgConn::connect(&cfg).await }
            })
        };
        let pool_handle = Self { config, pool };
        pool_handle.prewarm().await?;
        Ok(pool_handle)
    }

    async fn prewarm(&self) -> Result<(), String> {
        if self.config.min_conns == 0 {
            return Ok(());
        }
        for _ in 0..self.config.min_conns.min(self.config.max_conns) {
            let conn = self
                .pool
                .acquire(Some(self.config.connect_timeout), None)
                .await
                .map_err(|err| format!("prewarm failed: {err:?}"))?;
            drop(conn);
        }
        Ok(())
    }

    pub async fn acquire(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<AsyncPooled<PgConn>, AsyncAcquireError> {
        loop {
            let conn = self
                .pool
                .acquire(Some(self.config.max_wait), cancel)
                .await?;
            if let Some(max_idle) = self.config.max_idle {
                if conn.as_ref().idle_for() > max_idle {
                    conn.discard();
                    continue;
                }
            }
            if let Some(interval) = self.config.health_check_interval {
                if conn.as_ref().idle_for() > interval {
                    match conn.as_ref().ping().await {
                        Ok(()) => {
                            conn.as_ref().touch();
                        }
                        Err(_) => {
                            conn.discard();
                            continue;
                        }
                    }
                }
            }
            return Ok(conn);
        }
    }

    pub fn config(&self) -> &PgPoolConfig {
        &self.config
    }

    pub fn in_flight(&self) -> usize {
        self.pool.in_flight()
    }

    pub fn idle_count(&self) -> usize {
        self.pool.idle_count()
    }
}

fn build_tls_connector(config: &PgPoolConfig) -> Result<MakeRustlsConnect, String> {
    let mut roots = RootCertStore::empty();
    let certs = rustls_native_certs::load_native_certs()
        .map_err(|err| format!("failed to load native certs: {err}"))?;
    for cert in certs {
        roots.add(cert).map_err(|err| err.to_string())?;
    }
    if let Some(path) = config.ssl_root_cert.as_ref() {
        let pem = std::fs::read(path).map_err(|err| err.to_string())?;
        let mut cursor = std::io::Cursor::new(pem);
        let certs = rustls_pemfile::certs(&mut cursor)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| err.to_string())?;
        for cert in certs {
            roots.add(cert).map_err(|err| err.to_string())?;
        }
    }
    if roots.is_empty() {
        return Err("no root certificates available for TLS".to_string());
    }
    let tls_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(tls_config))
}
