// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Property based tests for bootstore scheme v0 protocol logic
//!
//! These tests operate by setting up a testbed of FSMs and triggering them
//! to exchange messages via API calls. The API calls are triggered via test
//! generated `Action`s. We check properties to validate correctness at various
//! stages of action processing such as after an FSM API gets called, and after
//! all generated messages have been delivered to the subset of reachable peers.

mod common;

use assert_matches::assert_matches;
use bootstore::schemes::v0::{
    ApiError, ApiOutput, Config, Envelope, Fsm, Msg, Output, Request,
    RequestType, Response, ResponseType, Ticks,
};

use proptest::prelude::*;
use sled_hardware::Baseboard;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use common::actions::{Action, Delays};
use common::generators::arb_test_input;
use common::network::Network;

/// State for the running test
///
/// `TestState` contains the real system under test (SUT) state of the peers, as
/// well as helper types and model state that allow making assertions about what
/// we expect the SUT to be at any given point in test execution.
pub struct TestState {
    // All peers in the test
    peers: BTreeMap<Baseboard, Fsm>,

    // A model of the network used for sending and receiving messages across
    // peers
    network: Network,

    // We assume all clocks tick at approximately the same rate, with a delta
    // small enough not to matter for the tests. We don't care about real time
    // synchronization at all. Therefore this clock, can serve as a global clock
    // and we can ensure that on each tick, a tick callback on every FSM fires.
    clock: Ticks,

    // A copy of the configuration at all peers
    config: Config,

    // Is the rack already initialized?
    //
    // If initialization fails, we have to wipe all the sleds and start over. For the
    // purposes of this test we therefore assume that initialization always succeeds
    // if the initialization action runs.
    rack_init_complete: bool,

    // Generated delays
    delays: Delays,
}

impl TestState {
    pub fn new(
        peer_ids: BTreeSet<Baseboard>,
        config: Config,
        delays: Delays,
    ) -> TestState {
        let peers = peer_ids
            .into_iter()
            .map(|id| (id.clone(), Fsm::new_uninitialized(id, config)))
            .collect();

        TestState {
            peers,
            network: Network::default(),
            clock: 0,
            config,
            rack_init_complete: false,
            delays,
        }
    }

    /// Process a test action
    pub fn on_action(&mut self, action: Action) -> Result<(), TestCaseError> {
        match action {
            Action::RackInit { rss_sled, rack_uuid, initial_members } => {
                self.on_rack_init(rss_sled, rack_uuid, initial_members)
            }
            Action::Connect(flows) => {
                // TODO: Assert that output makes sense and dispatch it
                for (source, dest) in flows {
                    self.network.connected(source.clone(), dest.clone());
                    let _output =
                        self.peer_mut(&source).connected(dest.clone());
                    let _output = self.peer_mut(&dest).connected(source);
                }
                Ok(())
            }
            Action::Disconnect(flows) => {
                // TODO: Assert that output makes sense and dispatch it
                for (source, dest) in flows {
                    self.network.disconnected(source.clone(), dest.clone());
                    let _output =
                        self.peer_mut(&source).disconnected(dest.clone());
                    let _output = self.peer_mut(&dest).disconnected(source);
                }
                Ok(())
            }
            Action::Ticks(ticks) => {
                for _ in 0..ticks {
                    self.clock += 1;
                    self.network.advance(self.clock);
                    let delivery_time = self.clock + self.delays.msg_delivery;
                    while let Some((destination, mut sourced_msgs)) =
                        self.network.delivered().pop_first()
                    {
                        for (source, msg) in sourced_msgs.drain(..) {
                            let output = self
                                .peer_mut(&destination)
                                .handle(source.clone(), msg);
                            // TODO: verify output
                            self.network.send(
                                &source,
                                output.envelopes,
                                delivery_time,
                            );
                        }
                    }
                }
                Ok(())
            }
            Action::ChangeDelays(delays) => {
                self.delays = delays;
                Ok(())
            }
            Action::LoadRackSecret(peer) => {
                // TODO: Verify output
                let output = self.peer_mut(&peer).load_rack_secret();
                let msg_delivery_time = self.clock + self.delays.msg_delivery;
                self.network.send(&peer, output.envelopes, msg_delivery_time);
                Ok(())
            }
        }
    }

    // Handle an `Action::RackInit`
    //
    // We always ensure the first rack init succeeds and so just run it to
    // completion instantly.
    //
    // The justification for this is that in a real system if rack init fails,
    // we have to start from scratch anyway by wiping all bootstores.
    fn on_rack_init(
        &mut self,
        rss_sled: Baseboard,
        rack_uuid: Uuid,
        initial_members: BTreeSet<Baseboard>,
    ) -> Result<(), TestCaseError> {
        let msg_delivery_time = self.clock + self.delays.msg_delivery;

        let output =
            self.peer_mut(&rss_sled).init_rack(rack_uuid, initial_members);
        self.check_rack_init_api_output(&rss_sled, &output)?;

        if self.rack_init_complete {
            // We already initialized the rack and verified an error
            // in `self.check_rack_init_api_output` above.
            // Just return here, as there is nothing left to do and the following code
            // is only meant for test startup.
            return Ok(());
        }

        // Send the `Initialize` messages to all peers
        self.network.send(&rss_sled, output.envelopes, msg_delivery_time);
        self.network.deliver_all();

        // Handle the `Initialize` message sent to each peer
        while let Some((destination, mut sourced_msgs)) =
            self.network.delivered().pop_first()
        {
            // There should only be one `Initialize` message sent to each peer
            prop_assert_eq!(sourced_msgs.len(), 1);
            let (source, msg) = sourced_msgs.pop_front().unwrap();
            let output = self.peer_mut(&destination).handle(source, msg);
            self.check_handle_initialize_req_output(&destination, &output)?;

            // Queue the acknowledgement to the rss_sled in the network
            self.network.send(
                &destination,
                output.envelopes,
                msg_delivery_time,
            );
        }

        // Deliver all the `InitAck` messages to the rss_sleds inbox
        self.network.deliver_all();

        while let Some((destination, sourced_msgs)) =
            self.network.delivered().pop_first()
        {
            // The only destination should be the rss_sled
            prop_assert_eq!(&rss_sled, &destination);
            // TODO: We only deal with initial members now. When
            // we have peers that will join later, we'll have to
            // check the actual initial members.
            prop_assert_eq!(sourced_msgs.len(), self.peers.len() - 1);

            let num_responses = sourced_msgs.len();
            for (i, (source, msg)) in sourced_msgs.into_iter().enumerate() {
                let output = self.peer_mut(&destination).handle(source, msg);
                if i == num_responses - 1 {
                    // Rack initialization completes on processing the
                    // last response and we inform the caller.
                    let expected: Output = ApiOutput::RackInitComplete.into();
                    prop_assert_eq!(expected, output);
                } else {
                    // Nothing happens until the rss_sled receives the
                    // last `InitAck` response.
                    prop_assert_eq!(Output::none(), output);
                }
            }
        }

        self.rack_init_complete = true;
        Ok(())
    }

    fn peer(&self, id: &Baseboard) -> &Fsm {
        self.peers.get(id).unwrap()
    }

    fn peer_mut(&mut self, id: &Baseboard) -> &mut Fsm {
        self.peers.get_mut(id).unwrap()
    }

    fn all_other_peers<'a>(
        &'a self,
        excluded: &'a Baseboard,
    ) -> impl Iterator<Item = &Baseboard> + 'a {
        self.peers.keys().filter(move |id| *id != excluded)
    }

    // Validate that the output of a rack_init API request made to a given
    // peer makes sense
    fn check_rack_init_api_output(
        &self,
        rss_sled: &Baseboard,
        output: &Output,
    ) -> Result<(), TestCaseError> {
        if self.rack_init_complete {
            let expected: Output = ApiError::RackAlreadyInitialized.into();
            prop_assert_eq!(&expected, output);
            Ok(())
        } else {
            prop_assert!(output.persist);
            prop_assert_eq!(&output.api_output, &None);
            for (peer, envelope) in
                self.all_other_peers(rss_sled).zip(&output.envelopes)
            {
                prop_assert_eq!(peer, &envelope.to);
            }
            Ok(())
        }
    }

    // Verify that the output of a peer receiving an `Initialize` request
    // is appropriate.
    fn check_handle_initialize_req_output(
        &self,
        peer_id: &Baseboard,
        output: &Output,
    ) -> Result<(), TestCaseError> {
        // We don't currently send extra rack init messages in this test
        prop_assert_eq!(self.peer(peer_id).state_name(), "initial_member");
        prop_assert!(output.persist);
        prop_assert_eq!(&output.api_output, &None);
        prop_assert_eq!(output.envelopes.len(), 1);
        let response_is_ack = matches!(
            output.envelopes[0].msg,
            Msg::Rsp(Response { request_id: _, type_: ResponseType::InitAck })
        );
        prop_assert!(response_is_ack);
        Ok(())
    }
}

proptest! {
    #[test]
    fn run(input in arb_test_input(12)) {
        let mut state = TestState::new(
            input.initial_members.clone(),
            input.config,
            Delays::default()
        );

        // Before we run our generated actions, we want to ensure all sleds are
        // connected to the rss_sled and successfully rack init. This is a requirement
        // monitored by humans on the real rack, so let's just do it.
        let rss_sled = input.initial_members.first().clone().unwrap();
        let flows = state.all_other_peers(&rss_sled).cloned().map(|dest| {
            (rss_sled.clone(), dest)
        }).collect();
        state.on_action(Action::Connect(flows))?;
        state.on_action(Action::RackInit {
            rss_sled: rss_sled.clone(),
            rack_uuid: input.rack_uuid,
            initial_members: input.initial_members.clone()
        })?;

        for action in input.actions {
            //println!("{:#?}", action);
            state.on_action(action)?;
        }
    }
}
