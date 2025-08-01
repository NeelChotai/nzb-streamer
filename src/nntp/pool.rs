use crate::nntp::{config::NntpConfig, error::NntpError};
use rek2_nntp::{authenticate, AuthenticatedConnection};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

pub struct ConnectionPool {
    config: NntpConfig,
    connections: Arc<Mutex<Vec<AuthenticatedConnection>>>,
    max_connections: usize,
}

impl ConnectionPool {
    pub fn new(config: NntpConfig) -> Self {
        let max_connections = config.max_connections.unwrap_or_else(|| {
            warn!("Max connections not provided. Using default (40)");
            40
        });

        Self {
            config,
            connections: Arc::new(Mutex::new(Vec::new())),
            max_connections,
        }
    }

    pub async fn get_connection(&self) -> Result<AuthenticatedConnection, NntpError> {
        let mut pool = self.connections.lock().await;

        if let Some(conn) = pool.pop() {
            debug!("Reusing connection from pool");
            Ok(conn)
        } else {
            debug!("Creating new NNTP connection");
            authenticate(
                &self.config.host,
                &self.config.username,
                &self.config.password,
            )
            .await
            .map_err(|e| NntpError::ClientAuthentication(e.to_string()))
        }
    }

    pub async fn return_connection(&self, conn: AuthenticatedConnection) {
        let mut pool = self.connections.lock().await;
        if pool.len() < self.max_connections {
            pool.push(conn);
            debug!("Returned connection to pool");
        } else {
            debug!("Pool full, dropping connection");
        }
    }
}
