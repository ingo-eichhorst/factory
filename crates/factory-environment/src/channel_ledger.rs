//! What each channel has sent (`#337`): the repeat ledger a send asks
//! before it goes, and writes once it went. One row per send, append-only --
//! the channel, the message's digest, the run, the time and the counts.
//! Never the text, never a file.

use chrono::{DateTime, Utc};
use factory_kernel::{FactoryError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const TABLE: &str = "channel_sends";

fn error(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("channel ledger sqlite", error.to_string())
}

/// One send that went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentRecord {
    pub id: String,
    pub channel: String,
    /// `channels::message_digest`: what makes two sends the same message.
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub sent_at: DateTime<Utc>,
    pub messages: usize,
    pub documents: usize,
    pub bytes: u64,
}

#[derive(Clone)]
pub struct ChannelLedger {
    conn: Arc<Mutex<Connection>>,
}

impl ChannelLedger {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path).map_err(error)?)
    }
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(error)?)
    }
    fn from_connection(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(error)?;
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (id TEXT PRIMARY KEY, channel TEXT NOT NULL, digest TEXT NOT NULL, sent_at TEXT NOT NULL, data TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS {TABLE}_digest ON {TABLE} (channel, digest, sent_at)"
        ))
        .map_err(error)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// The newest send of this message to this channel at or after `since`.
    pub async fn last_since(&self, channel: &str, digest: &str, since: DateTime<Utc>) -> Result<Option<SentRecord>> {
        let conn = self.conn.clone();
        let (channel, digest) = (channel.to_string(), digest.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(error)?;
            let data: Option<String> = conn
                .query_row(
                    &format!(
                        "SELECT data FROM {TABLE} WHERE channel = ?1 AND digest = ?2 AND sent_at >= ?3 ORDER BY sent_at DESC LIMIT 1"
                    ),
                    params![channel, digest, since.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)],
                    |row| row.get(0),
                )
                .optional()
                .map_err(error)?;
            data.map(|d| serde_json::from_str(&d).map_err(error)).transpose()
        })
        .await
        .map_err(error)?
    }

    pub async fn record(&self, record: &SentRecord) -> Result<()> {
        let conn = self.conn.clone();
        let record = record.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(error)?;
            conn.execute(
                &format!("INSERT INTO {TABLE} (id, channel, digest, sent_at, data) VALUES (?1, ?2, ?3, ?4, ?5)"),
                params![
                    record.id,
                    record.channel,
                    record.digest,
                    record.sent_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                    serde_json::to_string(&record).map_err(error)?
                ],
            )
            .map_err(error)?;
            Ok(())
        })
        .await
        .map_err(error)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: &str, digest: &str, at: DateTime<Utc>) -> SentRecord {
        SentRecord {
            id: id.into(),
            channel: "owner-telegram".into(),
            digest: digest.into(),
            run: Some("run-1".into()),
            task: Some("task-1".into()),
            sent_at: at,
            messages: 1,
            documents: 0,
            bytes: 5,
        }
    }

    #[tokio::test]
    async fn a_send_is_found_again_inside_the_window_and_not_before_it() {
        let ledger = ChannelLedger::in_memory().unwrap();
        let now = Utc::now();
        ledger.record(&sent("a", "d1", now - chrono::Duration::hours(13))).await.unwrap();
        assert_eq!(ledger.last_since("owner-telegram", "d1", now - chrono::Duration::hours(12)).await.unwrap(), None);
        ledger.record(&sent("b", "d1", now - chrono::Duration::hours(1))).await.unwrap();
        ledger.record(&sent("c", "d2", now)).await.unwrap();
        let found = ledger.last_since("owner-telegram", "d1", now - chrono::Duration::hours(12)).await.unwrap().unwrap();
        assert_eq!(found.id, "b");
        assert_eq!(ledger.last_since("other", "d1", now - chrono::Duration::hours(12)).await.unwrap(), None);
    }
}
