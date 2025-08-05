use crate::nntp::error::NntpPoolError;
use crate::nntp::{config::NntpConfig, error::NntpError};
use deadpool::Runtime;
use deadpool::managed::{Manager, Metrics, Pool, PoolConfig, QueueMode, RecycleResult, Timeouts};
use rek2_nntp::{AuthenticatedConnection, authenticate};
use shrinkwraprs::Shrinkwrap;
use std::{sync::Arc, time::Duration};
use tokio::time;
use tracing::{debug, info, warn};

pub struct Connection {
    config: NntpConfig,
}

impl Manager for Connection {
    type Type = AuthenticatedConnection;
    type Error = NntpPoolError;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        debug!("Creating new NNTP connection to {}", self.config.host);

        let conn = authenticate(
            &self.config.host,
            &self.config.username,
            &self.config.password,
        )
        .await
        .map_err(|e| NntpPoolError::Authentication(e.to_string()))?;

        Ok(conn)
    }

    async fn recycle(
        &self,
        _conn: &mut Self::Type,
        _metrics: &Metrics,
    ) -> RecycleResult<Self::Error> {
        // Could send NOOP here to check health
        Ok(())
    }
}

#[derive(Shrinkwrap)]
pub struct NntpPool(pub Pool<Connection>);

impl NntpPool {
    pub fn new(config: NntpConfig) -> Result<Self, NntpError> {
        let connection = Connection {
            config: config.clone(),
        };

        let pool_config = PoolConfig {
            max_size: *config.max_connections,
            timeouts: Timeouts {
                wait: None, // Block forever - caller shouldn't care about pool state
                create: Some(Duration::from_secs(5)), // TODO: revisit
                recycle: Some(*config.idle_timeout),
            },
            queue_mode: QueueMode::default(),
        };

        let pool = Pool::builder(connection)
            .config(pool_config)
            .runtime(Runtime::Tokio1)
            .build()?;

        Ok(NntpPool(pool))
    }

    pub async fn warm_pool(self: Arc<Self>) {
        let target = *self.0.manager().config.max_connections;
        info!("Pre-warming connection pool with {} connections", target);

        for i in 0..target {
            tokio::spawn({
                let client = Arc::clone(&self);
                async move {
                    time::sleep(Duration::from_millis(i as u64 * 50)).await;

                    match client.0.get().await {
                        Ok(_) => {
                            debug!("Pre-warmed connection {}", i);
                        }
                        Err(e) => {
                            warn!("Failed to pre-warm connection {}: {}", i, e);
                        }
                    };
                }
            });
        }

        info!("Connection pool warmed");
    }
}
