//! # AETHER Relay — Selective Forwarding Unit (SFU)
//!
//! The SFU receives encoded streams from publishers and forwards RTP packets
//! to subscribers without decoding. This is the core of scalable WebRTC-style
//! broadcasting.
//!
//! ## High-Concurrency Architecture
//!
//! We use a **lock-free** design with `dashmap` to map tracks to `tokio::sync::broadcast` channels.
//! This gives us:
//! 1. **O(1) Fan-out:** A broadcast channel uses a single contiguous ring buffer. When a publisher sends
//!    a packet, it pushes to the buffer *once*. 1,000 subscribers read from that same buffer.
//! 2. **Zero Global Locks:** Publishers can send packets concurrently without blocking each other.

use bytes::Bytes;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{info, trace};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("session not found: {0}")]
    SessionNotFound(Uuid),
    #[error("track not found: {0}")]
    TrackNotFound(String),
}

/// Unique identifier for a track (audio or video stream).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackId(pub String);

impl TrackId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// An active session — one publisher or subscriber connection.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: Uuid,
    pub is_publisher: bool,
}

impl Session {
    pub fn publisher() -> Self {
        Self {
            id: Uuid::new_v4(),
            is_publisher: true,
        }
    }
    pub fn subscriber() -> Self {
        Self {
            id: Uuid::new_v4(),
            is_publisher: false,
        }
    }
}

/// The Selective Forwarding Unit.
///
/// Thread-safe and lock-free router utilizing `DashMap` and `broadcast` channels.
pub struct Sfu {
    sessions: DashMap<Uuid, Session>,
    tracks: DashMap<TrackId, broadcast::Sender<Bytes>>,
}

impl Sfu {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: DashMap::new(),
            tracks: DashMap::new(),
        })
    }

    /// Registers a new session.
    pub fn add_session(&self, session: Session) -> Uuid {
        let id = session.id;
        info!(session_id = %id, publisher = session.is_publisher, "SFU: session registered");
        self.sessions.insert(id, session);
        id
    }

    /// Removes a session.
    pub fn remove_session(&self, id: Uuid) -> Result<(), RelayError> {
        if self.sessions.remove(&id).is_none() {
            return Err(RelayError::SessionNotFound(id));
        }
        info!(session_id = %id, "SFU: session removed");
        Ok(())
    }

    /// Subscribes to a track, returning a broadcast Receiver.
    /// If the track doesn't exist yet, it is created.
    pub fn subscribe(&self, track: &TrackId) -> broadcast::Receiver<Bytes> {
        let entry = self.tracks.entry(track.clone()).or_insert_with(|| {
            // Channel capacity of 1024 allows absorbing spikes without dropping slow readers
            let (tx, _) = broadcast::channel(1024);
            tx
        });
        info!(track = %track.0, "SFU: new subscriber added to track");
        entry.value().subscribe()
    }

    /// Receives an RTP packet from a publisher and routes it in O(1) to all subscribers.
    pub fn forward_rtp(&self, track: &TrackId, packet: Bytes) {
        if let Some(tx) = self.tracks.get(track) {
            // Note: `send` only fails if there are zero active receivers.
            // We don't care if no one is listening right now.
            if let Ok(count) = tx.send(packet.clone()) {
                trace!(track = %track.0, subs = count, "SFU: routed packet");
            }
        } else {
            // Publisher sending to a track that has no channel. Create it lazily.
            let (tx, _) = broadcast::channel(1024);
            // It's possible someone subscribed in between, so we use `or_insert`
            let tx = self.tracks.entry(track.clone()).or_insert(tx);
            let _ = tx.value().send(packet);
        }
    }

    /// Returns the total number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Returns the number of subscribers for a given track.
    pub fn subscriber_count(&self, track: &TrackId) -> usize {
        self.tracks
            .get(track)
            .map(|tx| tx.receiver_count())
            .unwrap_or(0)
    }
}

impl Default for Sfu {
    fn default() -> Self {
        Self {
            sessions: DashMap::new(),
            tracks: DashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_sfu_broadcast_fanout() {
        let sfu = Sfu::new();
        let track = TrackId::new("video-main");

        // 1. Subscribe multiple clients
        let mut sub1 = sfu.subscribe(&track);
        let mut sub2 = sfu.subscribe(&track);
        let mut sub3 = sfu.subscribe(&track);

        assert_eq!(sfu.subscriber_count(&track), 3);

        // 2. Publish a packet
        let packet = Bytes::from_static(b"RTP_PAYLOAD");
        sfu.forward_rtp(&track, packet.clone());

        // 3. All subscribers should receive the exact same packet without iteration
        assert_eq!(sub1.recv().await.unwrap(), packet);
        assert_eq!(sub2.recv().await.unwrap(), packet);
        assert_eq!(sub3.recv().await.unwrap(), packet);
    }

    #[tokio::test]
    async fn test_remove_session_cleans_up() {
        let sfu = Sfu::new();
        let session = Session::publisher();
        let id = sfu.add_session(session);
        assert_eq!(sfu.session_count(), 1);
        sfu.remove_session(id).unwrap();
        assert_eq!(sfu.session_count(), 0);
    }
}
