// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! V0 protocol state machine
//!
//! This state machine is entirely synchronous. It performs actions and returns
//! results. This is where the bulk of the protocol logic lives. It's
//! written this way to enable easy testing and auditing.

use super::share_pkg::{LearnedSharePkg, SharePkg};
use super::{Envelope, Msg, Request, RequestType};
use serde::{Deserialize, Serialize};
use sled_hardware::Baseboard;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::time::{Duration, Instant};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Configuration of the FSM
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub learn_timeout: Duration,
    pub rack_init_timeout: Duration,
    pub rack_secret_request_timeout: Duration,
}

/// An attempt by *this* peer to learn a key share
///
/// When received it triggers a `TrackableRequest::Learn` at the receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnAttempt {
    pub peer: Baseboard,
    pub expiry: Instant,
}

// An index into an encrypted share
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct ShareIdx(pub usize);

pub enum State {
    Uninitialized,
    InitialMember { pkg: SharePkg },
    Learning { attempt: Option<LearnAttempt> },
    Learned { pkg: LearnedSharePkg },
}

pub struct Fsm2 {
    /// The current state of this peer
    state: State,
    /// Unique IDs of this peer
    id: Baseboard,

    config: Config,

    /// Unique IDs of connected peers
    connected_peers: BTreeSet<Baseboard>,

    /// The approximate wall-clock time
    ///
    /// This is updated via API calls, and not read directly.
    /// Doing it this way allows deterministic tests.
    clock: Instant,

    /// Manage all trackable broadcasts
    request_manager: RequestManager,
}

impl Fsm2 {}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Share(Vec<u8>);

// Manually implemented to redact info
impl Debug for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Share").finish()
    }
}

/// Acknowledgement tracking for `RequestType::InitRack`.
#[derive(Debug, Default)]
pub struct InitAcks {
    expected: BTreeSet<Baseboard>,
    received: BTreeSet<Baseboard>,
}

/// Acknowledgement tracking for `RequestType::LoadRackSecret` and
/// `RequestType::Learn`
#[derive(Debug)]
pub struct ShareAcks {
    threshold: u8,
    received: BTreeMap<Baseboard, Share>,
}

impl ShareAcks {
    pub fn new(threshold: u8) -> ShareAcks {
        ShareAcks { threshold, received: BTreeMap::new() }
    }
}

/// A mechanism to track in flight requests
///
/// Manages expiry, acknowledgment, and retries
#[derive(Debug)]
pub enum TrackableRequest {
    /// A request from the caller of the Fsm API to initialize a rack
    ///
    /// This must only be called at one peer, exactly once.
    InitRack {
        rack_uuid: Uuid,
        packages: BTreeMap<Baseboard, SharePkg>,
        acks: InitAcks,
    },

    /// A request from the caller of the Fsm API to load a rack secret
    LoadRackSecret { rack_uuid: Uuid, acks: ShareAcks },

    /// A request from a peer to learn a new share
    //
    /// This peer was not part of the initial membership group.
    Learn { rack_uuid: Uuid, from: Baseboard, acks: ShareAcks },
}

/// A mechanism to manage all in flight requests
///
/// We expect very few requests at a time - on the order of one or two requests.
pub struct RequestManager {
    config: Config,
    requests: BTreeMap<Uuid, TrackableRequest>,
    expiry_to_id: BTreeMap<Instant, Uuid>,
}

impl RequestManager {
    pub fn new_init_rack(
        &mut self,
        now: Instant,
        rack_uuid: Uuid,
        packages: BTreeMap<Baseboard, SharePkg>,
    ) -> Uuid {
        let expiry = now + self.config.rack_init_timeout;
        let req = TrackableRequest::InitRack {
            rack_uuid,
            packages,
            acks: InitAcks::default(),
        };
        self.new_request(expiry, req)
    }

    pub fn new_load_rack_secret(
        &mut self,
        now: Instant,
        rack_uuid: Uuid,
        threshold: u8,
    ) -> Uuid {
        let expiry = now + self.config.rack_secret_request_timeout;
        self.new_request(
            expiry,
            TrackableRequest::LoadRackSecret {
                rack_uuid,
                acks: ShareAcks::new(threshold),
            },
        )
    }

    pub fn new_learn(
        &mut self,
        now: Instant,
        rack_uuid: Uuid,
        threshold: u8,
        from: Baseboard,
    ) -> Uuid {
        let expiry = now + self.config.learn_timeout;
        self.new_request(
            expiry,
            TrackableRequest::Learn {
                rack_uuid,
                from,
                acks: ShareAcks::new(threshold),
            },
        )
    }

    fn new_request(
        &mut self,
        expiry: Instant,
        request: TrackableRequest,
    ) -> Uuid {
        let id = Uuid::new_v4();
        self.requests.insert(id, request);
        self.expiry_to_id.insert(expiry, id);
        id
    }

    /// Return any expired requests
    ///
    /// This is typically called during `on_tick` callbacks.
    pub fn expired(&mut self, now: Instant) -> Vec<TrackableRequest> {
        let mut expired = vec![];
        while let Some((expiry, request_id)) = self.expiry_to_id.pop_last() {
            if expiry > now {
                expired.push(self.requests.remove(&request_id).unwrap());
            } else {
                // Put the last request back. We are done.
                self.expiry_to_id.insert(expiry, request_id);
                break;
            }
        }
        expired
    }

    /// Return true if initialization completed, false otherwise
    ///
    /// If initialization completed, the request will be deleted.
    ///
    /// We drop the ack if the request_id is not found. This could be a lingering
    /// old ack from when the rack was reset to clean up after a prior failed rack
    /// init.
    pub fn on_init_ack(&mut self, from: Baseboard, request_id: Uuid) -> bool {
        if let Some(TrackableRequest::InitRack { acks, .. }) =
            self.requests.get_mut(&request_id)
        {
            acks.received.insert(from);
            if acks.received == acks.expected {
                self.requests.remove(&request_id);
                return true;
            }
        }

        false
    }

    /// Return the `Some(request)` if a threshold of acks has been received.
    /// Otherwise return `None`
    pub fn on_share(
        &mut self,
        from: Baseboard,
        request_id: Uuid,
        share: Share,
    ) -> Option<TrackableRequest> {
        let acks = match self.requests.get_mut(&request_id) {
            Some(TrackableRequest::LoadRackSecret { acks, .. }) => acks,
            Some(TrackableRequest::Learn { acks, .. }) => acks,
            _ => return None,
        };
        acks.received.insert(from, share);
        if acks.received.len() == acks.threshold as usize {
            self.requests.remove(&request_id)
        } else {
            None
        }
    }

    /// If there are outstanding requests and this peer has not acknowledged
    /// the given request then send the request to the peer.
    pub fn on_connected(&self, peer_id: &Baseboard) -> Vec<Envelope> {
        let mut envelopes = vec![];
        for (request_id, request) in &self.requests {
            match request {
                TrackableRequest::InitRack { rack_uuid, packages, acks } => {
                    if acks.received.contains(peer_id) {
                        continue;
                    }
                    if let Some(pkg) = packages.get(peer_id) {
                        envelopes.push(Envelope {
                            to: peer_id.clone(),
                            msg: Msg::Req(Request {
                                id: *request_id,
                                type_: RequestType::Init(pkg.clone()),
                            }),
                        });
                    }
                }
                TrackableRequest::LoadRackSecret { rack_uuid, acks } => {
                    if acks.received.contains_key(peer_id) {
                        continue;
                    }
                    envelopes.push(Envelope {
                        to: peer_id.clone(),
                        msg: Msg::Req(Request {
                            id: *request_id,
                            type_: RequestType::GetShare {
                                rack_uuid: *rack_uuid,
                            },
                        }),
                    });
                }
                TrackableRequest::Learn { rack_uuid, acks, .. } => {
                    if acks.received.contains_key(peer_id) {
                        continue;
                    }
                    envelopes.push(Envelope {
                        to: peer_id.clone(),
                        msg: Msg::Req(Request {
                            id: *request_id,
                            type_: RequestType::GetShare {
                                rack_uuid: *rack_uuid,
                            },
                        }),
                    });
                }
            }
        }
        envelopes
    }
}
