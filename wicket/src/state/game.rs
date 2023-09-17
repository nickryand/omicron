// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The state of our game screen
//!
//! We store the state in the global state struct so that
//! we can use the replay debugger.

use ratatui::prelude::Rect;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// The state of our [`crate::ui::game::GameScreen`]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameState {
    pub delivery: SpecialDelivery,
}

impl GameState {
    pub fn new() -> GameState {
        GameState { delivery: SpecialDelivery::new() }
    }
}

///
/// The state for the game "Special Delivery"
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecialDelivery {
    // Time from the start of the game in ms
    pub now_ms: u64,
    pub rect: Rect,
    pub racks_remaining: u32,
    pub racks_delivered: u32,
    pub trucks: Vec<Truck>,
    pub racks: Vec<Rack>,
    // The user controlled position of the rack to be dropped
    pub dropper_pos: u16,
}

impl SpecialDelivery {
    fn new() -> SpecialDelivery {
        SpecialDelivery {
            now_ms: 0,
            rect: Rect::default(),
            racks_remaining: 10,
            racks_delivered: 0,
            trucks: Vec::new(),
            racks: Vec::new(),
            dropper_pos: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HorizontalDirection {
    Left,
    Right,
}

// Truck position of front bumper = travel_time_ms * (speed / 1000)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Truck {
    // position of front bumper = travel_time_ms * speed
    pub position: u16,
    pub travel_time_ms: u32,
    pub bed_width: u16,
    pub speed: f32, // cells/ms
}

impl Truck {
    // All trucks start with the front bumper visible from the left side of
    // the screen.
    pub fn new(bed_width: u16, cells_per_sec: f32) -> Truck {
        let speed = cells_per_sec / 1000.0;
        Truck { position: 0, travel_time_ms: 0, speed, bed_width }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rack {
    rect: Rect,
}
