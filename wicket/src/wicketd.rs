// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Code for talking to wicketd

use slog::{o, warn, Logger};
use std::convert::From;
use std::net::SocketAddrV6;
use tokio::sync::mpsc::{self, Sender, UnboundedSender};
use tokio::time::{interval, Duration, Instant, MissedTickBehavior};
use wicketd_client::types::{RackV1Inventory, SpIdentifier, SpType};
use wicketd_client::GetInventoryResponse;

use crate::state::ComponentId;
use crate::{Event, InventoryEvent};

impl From<ComponentId> for SpIdentifier {
    fn from(id: ComponentId) -> Self {
        match id {
            ComponentId::Sled(i) => {
                SpIdentifier { type_: SpType::Sled, slot: i as u32 }
            }
            ComponentId::Psc(i) => {
                SpIdentifier { type_: SpType::Power, slot: i as u32 }
            }
            ComponentId::Switch(i) => {
                SpIdentifier { type_: SpType::Switch, slot: i as u32 }
            }
        }
    }
}

const WICKETD_POLL_INTERVAL: Duration = Duration::from_millis(500);
const WICKETD_TIMEOUT: Duration = Duration::from_millis(1000);

// Assume that these requests are periodic on the order of seconds or the
// result of human interaction. In either case, this buffer should be plenty
// large.
const CHANNEL_CAPACITY: usize = 1000;

/// Requests driven by the UI and sent from [`crate::Runner`] to [`WicketdManager`]
#[allow(unused)]
#[derive(Debug)]
pub enum Request {
    StartUpdate(ComponentId),
}

pub struct WicketdHandle {
    pub tx: Sender<Request>,
}

/// Wrapper around Wicketd clients used to poll inventory
/// and perform updates.
pub struct WicketdManager {
    log: Logger,
    rx: mpsc::Receiver<Request>,
    events_tx: UnboundedSender<Event>,
    wicketd_addr: SocketAddrV6,
}

impl WicketdManager {
    pub fn new(
        log: &Logger,
        events_tx: UnboundedSender<Event>,
        wicketd_addr: SocketAddrV6,
    ) -> (WicketdHandle, WicketdManager) {
        let log = log.new(o!("component" => "WicketdManager"));
        let (tx, rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
        let handle = WicketdHandle { tx };
        let manager = WicketdManager { log, rx, events_tx, wicketd_addr };

        (handle, manager)
    }

    /// Manage interactions with wicketd on the same scrimlet
    ///
    /// * Send requests to wicketd
    /// * Receive responses / errors
    /// * Translate any responses/errors into [`Event`]s
    ///   that can be utilized by the UI.
    pub async fn run(mut self) {
        self.poll_inventory().await;
        self.poll_update_log().await;
        self.poll_artifacts().await;

        loop {
            tokio::select! {
                Some(request) = self.rx.recv() => {
                    slog::info!(self.log, "Got wicketd req: {:?}", request);
                    let Request::StartUpdate(component_id) = request;
                    self.start_update(component_id).await;
                }
                else => {
                    slog::info!(self.log, "Request receiver closed. Process must be exiting.");
                    break;
                }
            }
        }
    }

    async fn start_update(&self, component_id: ComponentId) {
        let log = self.log.clone();
        let tx = self.events_tx.clone();
        let addr = self.wicketd_addr;
        tokio::spawn(async move {
            let update_client =
                create_wicketd_client(&log, addr, WICKETD_TIMEOUT);
            let sp: SpIdentifier = component_id.into();
            let res = update_client.post_start_update(sp.type_, sp.slot).await;
            // We don't return errors or success values, as there's nobody to
            // return them to. Instead, all updates are periodically polled
            // and global state mutated. This allows the update pane to
            // report current status to users in a more detailed and holistic
            // fashion.
            slog::info!(log, "Update response for {}: {:?}", component_id, res);
        });
    }

    async fn poll_artifacts(&self) {
        let log = self.log.clone();
        let tx = self.events_tx.clone();
        let addr = self.wicketd_addr;
        tokio::spawn(async move {
            let client = create_wicketd_client(&log, addr, WICKETD_TIMEOUT);
            let mut ticker = interval(WICKETD_POLL_INTERVAL * 2);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                // TODO: We should really be using ETAGs here
                match client.get_artifacts().await {
                    Ok(val) => {
                        // TODO: Only send on changes
                        let artifacts = val.into_inner().artifacts;
                        let _ = tx.send(Event::UpdateArtifacts(artifacts));
                    }
                    Err(e) => {
                        warn!(log, "{e}");
                    }
                }
            }
        });
    }

    async fn poll_update_log(&self) {
        let log = self.log.clone();
        let tx = self.events_tx.clone();
        let addr = self.wicketd_addr;

        tokio::spawn(async move {
            let client = create_wicketd_client(&log, addr, WICKETD_TIMEOUT);
            let mut ticker = interval(WICKETD_POLL_INTERVAL * 2);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                // TODO: We should really be using ETAGs here
                match client.get_update_all().await {
                    Ok(val) => {
                        // TODO: Only send on changes
                        let logs = val.into_inner();
                        let _ = tx.send(Event::UpdateLog(logs));
                    }
                    Err(e) => {
                        warn!(log, "{e}");
                    }
                }
            }
        });
    }

    async fn poll_inventory(&self) {
        let log = self.log.clone();
        let tx = self.events_tx.clone();
        let addr = self.wicketd_addr;

        let mut state = InventoryState::new(&log, tx);

        tokio::spawn(async move {
            let client = create_wicketd_client(&log, addr, WICKETD_TIMEOUT);
            let mut ticker = interval(WICKETD_POLL_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                // TODO: We should really be using ETAGs here
                match client.get_inventory().await {
                    Ok(val) => {
                        let new_inventory = val.into_inner();
                        state.send_if_changed(new_inventory.into()).await;
                    }
                    Err(e) => {
                        warn!(log, "{e}");
                    }
                }
            }
        });
    }
}

pub(crate) fn create_wicketd_client(
    log: &Logger,
    wicketd_addr: SocketAddrV6,
    timeout: Duration,
) -> wicketd_client::Client {
    let endpoint =
        format!("http://[{}]:{}", wicketd_addr.ip(), wicketd_addr.port());
    let client = reqwest::ClientBuilder::new()
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
        .unwrap();

    wicketd_client::Client::new_with_client(&endpoint, client, log.clone())
}

#[derive(Debug)]
struct InventoryState {
    log: Logger,
    current_inventory: Option<RackV1Inventory>,
    tx: UnboundedSender<Event>,
}

impl InventoryState {
    fn new(log: &Logger, tx: UnboundedSender<Event>) -> Self {
        let log = log.new(o!("component" => "InventoryState"));
        Self { log, current_inventory: None, tx }
    }

    async fn send_if_changed(&mut self, new_inventory: GetInventoryResponse) {
        match (self.current_inventory.take(), new_inventory) {
            (
                current_inventory,
                GetInventoryResponse::Response {
                    inventory: new_inventory,
                    received_ago: mgs_received_ago,
                },
            ) => {
                let changed_inventory = (current_inventory.as_ref()
                    != Some(&new_inventory))
                .then(|| {
                    self.current_inventory = Some(new_inventory.clone());
                    new_inventory
                });

                let _ =
                    self.tx.send(Event::Inventory(InventoryEvent::Inventory {
                        changed_inventory,
                        wicketd_received: Instant::now(),
                        mgs_received: libsw::TokioSw::with_elapsed_started(
                            mgs_received_ago,
                        ),
                    }));
            }
            (Some(_), GetInventoryResponse::Unavailable) => {
                // This is an illegal state transition -- wicketd can never return Unavailable after
                // returning a response.
                slog::error!(
                    self.log,
                    "Illegal state transition from response to unavailable"
                );
            }
            (None, GetInventoryResponse::Unavailable) => {
                // No response received by wicketd from MGS yet.
                let _ = self.tx.send(Event::Inventory(
                    InventoryEvent::Unavailable {
                        wicketd_received: Instant::now(),
                    },
                ));
            }
        };
    }
}
