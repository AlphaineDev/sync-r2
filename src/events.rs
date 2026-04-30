use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub event_type: String,
    pub timestamp: String,
    pub data: Value,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<Event>,
}

impl EventHub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn emit(&self, event_type: impl Into<String>, data: Value, message: Option<String>) {
        let _ = self.tx.send(Event {
            event_type: event_type.into(),
            timestamp: Utc::now().to_rfc3339(),
            data,
            message,
        });
    }
}
