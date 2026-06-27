//! # AETHER Relay — Selective Forwarding Unit (SFU)
//!
//! The SFU receives encoded streams from publishers and forwards RTP packets
//! to subscribers without decoding. This is the core of scalable WebRTC-style
//! broadcasting: the server never touches the media, only the network packets.
//!
//! ## Architecture
//!
//! ```text
//!  Publisher A ──→ Session(pub_id) ──→ ForwardingTable
//!  Publisher B ──→ Session(pub_id) ──╱         │
//!                                              ▼
//!                                   Subscriber 1, 2, 3 ...
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use bytes::Bytes;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;
use tracing::{debug, info, warn};

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("session not found: {0}")]
    SessionNotFound(Uuid),
    #[error("track not found: {0}")]
    TrackNotFound(String),
    #[error("subscriber already registered")]
    DuplicateSubscriber,
}

/// Unique identifier for a track (audio or video stream).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackId(pub String);

impl TrackId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
}

/// A subscriber's packet receiver.
/// The SFU sends cloned packet bytes into this channel.
pub type SubscriberTx = mpsc::Sender<Bytes>;

/// An active session — one publisher or subscriber connection.
#[derive(Debug)]
pub struct Session {
    pub id: Uuid,
    pub tracks: Vec<TrackId>,
    pub is_publisher: bool,
    /// Outbound channel for subscribers (None for publishers)
    pub tx: Option<SubscriberTx>,
}

impl Session {
    pub fn publisher(tracks: Vec<TrackId>) -> Self {
        Self { id: Uuid::new_v4(), tracks, is_publisher: true, tx: None }
    }
    pub fn subscriber(tracks: Vec<TrackId>, tx: SubscriberTx) -> Self {
        Self { id: Uuid::new_v4(), tracks, is_publisher: false, tx: Some(tx) }
    }
}

/// Maps track IDs to the set of subscriber channels currently watching them.
#[derive(Default)]
pub struct ForwardingTable {
    inner: HashMap<TrackId, Vec<SubscriberTx>>,
}

impl ForwardingTable {
    pub fn subscribe(&mut self, track: TrackId, tx: SubscriberTx) {
        self.inner.entry(track).or_default().push(tx);
    }

    pub fn unsubscribe(&mut self, track: &TrackId, tx: &SubscriberTx) {
        if let Some(subs) = self.inner.get_mut(track) {
            // Compare by pointer identity of the channel sender
            subs.retain(|s| !s.same_channel(tx));
        }
    }

    /// Forwards a packet to all subscribers of `track`.
    /// Dead subscribers (closed channels) are pruned.
    pub async fn forward(&mut self, track: &TrackId, packet: Bytes) {
        if let Some(subs) = self.inner.get_mut(track) {
            let mut dead = Vec::new();
            for (i, tx) in subs.iter().enumerate() {
                if tx.try_send(packet.clone()).is_err() {
                    dead.push(i);
                }
            }
            for i in dead.into_iter().rev() {
                warn!(track = %track.0, sub_index = i, "subscriber dead, pruning");
                subs.swap_remove(i);
            }
        }
    }

    pub fn subscriber_count(&self, track: &TrackId) -> usize {
        self.inner.get(track).map(|v| v.len()).unwrap_or(0)
    }
}

/// The Selective Forwarding Unit.
///
/// Manages sessions and owns the forwarding table. Thread-safe via RwLock.
pub struct Sfu {
    sessions: RwLock<HashMap<Uuid, Session>>,
    table: RwLock<ForwardingTable>,
}

impl Sfu {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            table: RwLock::new(ForwardingTable::default()),
        })
    }

    /// Registers a new session (publisher or subscriber).
    pub async fn add_session(&self, session: Session) -> Uuid {
        let id = session.id;
        if !session.is_publisher {
            if let Some(ref tx) = session.tx {
                let mut table = self.table.write().await;
                for track in &session.tracks {
                    table.subscribe(track.clone(), tx.clone());
                }
            }
        }
        info!(session_id = %id, publisher = session.is_publisher, "SFU: session registered");
        self.sessions.write().await.insert(id, session);
        id
    }

    /// Removes a session and cleans up forwarding table entries.
    pub async fn remove_session(&self, id: Uuid) -> Result<(), RelayError> {
        let session = self.sessions.write().await.remove(&id)
            .ok_or(RelayError::SessionNotFound(id))?;
        if !session.is_publisher {
            if let Some(ref tx) = session.tx {
                let mut table = self.table.write().await;
                for track in &session.tracks {
                    table.unsubscribe(track, tx);
                }
            }
        }
        info!(session_id = %id, "SFU: session removed");
        Ok(())
    }

    /// Receives an RTP packet from a publisher and forwards to all subscribers.
    pub async fn forward_rtp(&self, track: &TrackId, packet: Bytes) {
        self.table.write().await.forward(track, packet).await;
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }
}

impl Default for Sfu {
    fn default() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            table: RwLock::new(ForwardingTable::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sfu_forward_to_subscriber() {
        let sfu = Sfu::new();
        let track = TrackId::new("video-main");
        let (tx, mut rx) = mpsc::channel(16);
        let sub = Session::subscriber(vec![track.clone()], tx);
        sfu.add_session(sub).await;
        let packet = Bytes::from_static(b"RTP_PAYLOAD");
        sfu.forward_rtp(&track, packet.clone()).await;
        let received = rx.recv().await.unwrap();
        assert_eq!(received, packet);
    }

    #[tokio::test]
    async fn test_remove_session_cleans_up() {
        let sfu = Sfu::new();
        let track = TrackId::new("audio");
        let (tx, _rx) = mpsc::channel(16);
        let sub = Session::subscriber(vec![track.clone()], tx);
        let id = sfu.add_session(sub).await;
        assert_eq!(sfu.session_count().await, 1);
        sfu.remove_session(id).await.unwrap();
        assert_eq!(sfu.session_count().await, 0);
    }
}
