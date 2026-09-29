//! Channel-2 demultiplexing for concurrent signing sessions.
//!
//! One reader owns the schnorr receiver and hands each message to the session whose height it
//! carries, so sessions running side by side never consume each other's commits or partials.
//! A session registers its height before its first request goes out and deregisters when it
//! ends; traffic for a height nobody holds is late or stale and is dropped.

use commonware_avs_core::bn254::PublicKey;
use commonware_codec::DecodeExt;
use commonware_p2p::Receiver;
use gas_killer_common::schnorr::wire::SchnorrMsg;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::{debug, info, warn};

/// Messages one session can have waiting before later ones are dropped. An honest operator sends
/// a session a handful per attempt, so only a flood fills it, and dropping the excess costs
/// that operator's contribution rather than the reader's memory.
const INBOX_CAPACITY: usize = 256;

type Delivery = (PublicKey, SchnorrMsg);

/// Height → the live session's inbox. `None` once the receiver has closed, so no session can
/// open after the network went down.
type Routes = Option<HashMap<u64, mpsc::Sender<Delivery>>>;

/// Hands out per-session inboxes over one shared channel-2 receiver. Cloning shares the routes.
#[derive(Clone)]
pub struct SessionInboxes {
    routes: Arc<Mutex<Routes>>,
}

impl SessionInboxes {
    /// Spawns the reader over `receiver`, delivering only messages from `operators`.
    pub fn spawn<R>(receiver: R, operators: HashSet<PublicKey>) -> Self
    where
        R: Receiver<PublicKey = PublicKey>,
    {
        let inboxes = Self {
            routes: Arc::new(Mutex::new(Some(HashMap::new()))),
        };
        tokio::spawn(inboxes.clone().route(receiver, operators));
        inboxes
    }

    /// Opens the inbox for the session at `height`, or `None` once the channel has closed.
    pub fn open(&self, height: u64) -> Option<Inbox> {
        let (sender, receiver) = mpsc::channel(INBOX_CAPACITY);
        let mut routes = self.lock();
        let previous = routes.as_mut()?.insert(height, sender);
        if previous.is_some() {
            warn!(
                height,
                "two sessions opened the same height; the older one goes deaf"
            );
        }
        Some(Inbox {
            height,
            receiver,
            routes: Arc::clone(&self.routes),
            closed: false,
        })
    }

    async fn route<R>(self, mut receiver: R, operators: HashSet<PublicKey>)
    where
        R: Receiver<PublicKey = PublicKey>,
    {
        loop {
            let (peer, bytes) = match receiver.recv().await {
                Ok(message) => message,
                Err(error) => {
                    info!(
                        ?error,
                        "schnorr channel closed; closing every session inbox"
                    );
                    // Dropping the senders wakes every session with a closed inbox.
                    *self.lock() = None;
                    return;
                }
            };
            if !operators.contains(&peer) {
                warn!(peer = %peer, "schnorr message from unknown peer; ignored");
                continue;
            }
            let message = match SchnorrMsg::decode(bytes) {
                Ok(message) => message,
                Err(error) => {
                    warn!(%error, "malformed schnorr message; ignored");
                    continue;
                }
            };
            let height = message.height();
            let Some(inbox) = self
                .lock()
                .as_ref()
                .and_then(|routes| routes.get(&height).cloned())
            else {
                debug!(height, "schnorr message for no live session; dropped");
                continue;
            };
            if let Err(TrySendError::Full(_)) = inbox.try_send((peer, message)) {
                warn!(height, "session inbox full; schnorr message dropped");
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Routes> {
        lock(&self.routes)
    }
}

/// A route table stays consistent under a panicking holder: every write is a single insert,
/// remove or reset.
fn lock(routes: &Mutex<Routes>) -> MutexGuard<'_, Routes> {
    routes.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One session's share of channel 2. Dropping it deregisters the height.
pub struct Inbox {
    height: u64,
    receiver: mpsc::Receiver<Delivery>,
    routes: Arc<Mutex<Routes>>,
    closed: bool,
}

impl Inbox {
    /// The next message for this session before `deadline`, or `None` on timeout or once the
    /// channel has closed ([`Self::closed`]).
    pub async fn recv_until(&mut self, deadline: Instant) -> Option<Delivery> {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        match tokio::time::timeout(remaining, self.receiver.recv()).await {
            Ok(Some(delivery)) => Some(delivery),
            Ok(None) => {
                self.closed = true;
                None
            }
            Err(_) => None,
        }
    }

    /// Whether the channel has closed, so no session can finish.
    pub fn closed(&self) -> bool {
        self.closed
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        if let Some(routes) = lock(&self.routes).as_mut() {
            routes.remove(&self.height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_avs_core::bn254::Bn254;
    use commonware_codec::Encode;
    use commonware_cryptography::Signer as _;
    use commonware_p2p::Message;
    use gas_killer_common::task_data::GasKillerTaskData;
    use std::time::Duration;

    /// A receiver fed by the test; closes when the test drops its sender.
    #[derive(Debug)]
    struct ScriptedReceiver(mpsc::UnboundedReceiver<Message<PublicKey>>);

    impl Receiver for ScriptedReceiver {
        type Error = std::io::Error;
        type PublicKey = PublicKey;

        async fn recv(&mut self) -> Result<Message<PublicKey>, Self::Error> {
            self.0
                .recv()
                .await
                .ok_or_else(|| std::io::Error::other("channel closed"))
        }
    }

    fn peer(seed: u64) -> PublicKey {
        Bn254::from_seed(seed).public_key()
    }

    fn message(height: u64) -> SchnorrMsg {
        SchnorrMsg::CommitRequest {
            height,
            attempt: 1,
            task: GasKillerTaskData::default(),
        }
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }

    fn network() -> (mpsc::UnboundedSender<Message<PublicKey>>, SessionInboxes) {
        let (wire, receiver) = mpsc::unbounded_channel();
        let inboxes = SessionInboxes::spawn(ScriptedReceiver(receiver), HashSet::from([peer(1)]));
        (wire, inboxes)
    }

    #[tokio::test]
    async fn interleaved_traffic_reaches_the_session_it_names() {
        let (wire, inboxes) = network();
        let mut first = inboxes.open(10).unwrap();
        let mut second = inboxes.open(11).unwrap();

        for height in [11, 10, 11] {
            wire.send((peer(1), message(height).encode().into()))
                .unwrap();
        }

        assert_eq!(first.recv_until(soon()).await.unwrap().1, message(10));
        assert_eq!(second.recv_until(soon()).await.unwrap().1, message(11));
        assert_eq!(second.recv_until(soon()).await.unwrap().1, message(11));
        let quiet = Instant::now() + Duration::from_millis(50);
        assert!(first.recv_until(quiet).await.is_none());
        assert!(!first.closed());
    }

    #[tokio::test]
    async fn strangers_and_ended_sessions_are_not_delivered() {
        let (wire, inboxes) = network();
        let mut live = inboxes.open(10).unwrap();
        drop(inboxes.open(9).unwrap());

        wire.send((peer(2), message(10).encode().into())).unwrap();
        wire.send((peer(1), message(9).encode().into())).unwrap();
        wire.send((peer(1), message(10).encode().into())).unwrap();

        let (from, received) = live.recv_until(soon()).await.unwrap();
        assert_eq!((from, received), (peer(1), message(10)));
        let quiet = Instant::now() + Duration::from_millis(50);
        assert!(live.recv_until(quiet).await.is_none());
    }

    #[tokio::test]
    async fn a_closed_channel_closes_every_inbox_and_refuses_new_ones() {
        let (wire, inboxes) = network();
        let mut live = inboxes.open(10).unwrap();

        drop(wire);

        assert!(live.recv_until(soon()).await.is_none());
        assert!(live.closed());
        tokio::time::timeout(Duration::from_secs(1), async {
            while inboxes.open(11).is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("no session opens once the channel closed");
    }
}
