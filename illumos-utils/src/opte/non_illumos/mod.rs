// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Mock / dummy versions of the OPTE module, for non-illumos platforms

use slog::Logger;

mod port_manager;

pub use port_manager::PortManager;
pub use port_manager::PortTicket;

use crate::addrobj::AddrObject;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Invalid IP configuration for OPTE port")]
    InvalidPortIpConfig,
}

pub fn initialize_xde_driver(
    log: &Logger,
    _underlay_nics: &[AddrObject],
) -> Result<(), Error> {
    slog::warn!(log, "`xde` driver is a fiction on non-illumos systems");
    Ok(())
}

pub fn delete_all_xde_devices(log: &Logger) -> Result<(), Error> {
    slog::warn!(log, "`xde` driver is a fiction on non-illumos systems");
    Ok(())
}
