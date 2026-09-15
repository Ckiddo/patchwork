use super::{Database, StoreError};
use futures_util::future::BoxFuture;
use sqlx::PgConnection;
use std::time::Duration;

pub enum TxError {
    Sql(sqlx::Error),
    Domain(StoreError),
}
impl From<sqlx::Error> for TxError {
    fn from(e: sqlx::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<StoreError> for TxError {
    fn from(e: StoreError) -> Self {
        Self::Domain(e)
    }
}

/// Only constructed after an acknowledged COMMIT (or a locked receipt read).
#[derive(Debug)]
pub struct Committed<T>(T);
impl<T> Committed<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}
fn retryable(e: &TxError) -> bool {
    matches!(e, TxError::Sql(sqlx::Error::Database(db)) if matches!(db.code().as_deref(), Some("40001" | "40P01")))
}
fn sanitize(e: TxError) -> StoreError {
    match e {
        TxError::Domain(e) => e,
        TxError::Sql(sqlx::Error::Database(db)) => match db.code().as_deref() {
            Some("23505" | "23503" | "23514" | "23502") => StoreError::Conflict,
            Some("42501") => StoreError::Permission,
            _ => StoreError::Unavailable,
        },
        _ => StoreError::Unavailable,
    }
}
impl Database {
    /// Closure must contain database work only: it may be retried after rollback.
    pub async fn transaction<T, F>(&self, mut work: F) -> Result<Committed<T>, StoreError>
    where
        T: Send,
        F: for<'c> FnMut(&'c mut PgConnection) -> BoxFuture<'c, Result<T, TxError>>,
    {
        for attempt in 0..=3 {
            let mut tx = self
                .pool()
                .begin()
                .await
                .map_err(|_| StoreError::Unavailable)?;
            match work(&mut tx).await {
                Ok(value) => {
                    tx.commit().await.map_err(|e| match e {
                        sqlx::Error::Database(ref error)
                            if matches!(
                                error.code().as_deref(),
                                Some("23502" | "23503" | "23505" | "23514" | "40001" | "40P01")
                            ) =>
                        {
                            sanitize(TxError::Sql(e))
                        }
                        _ => StoreError::CommitUnknown,
                    })?;
                    return Ok(Committed(value));
                }
                Err(error) => {
                    let retry = retryable(&error) && attempt < 3;
                    tx.rollback().await.map_err(|_| StoreError::Unavailable)?;
                    if !retry {
                        return Err(sanitize(error));
                    }
                    let delay = 10 * (1 << attempt) + rand::random_range(0..10);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
        unreachable!("bounded retry loop always returns")
    }
}
