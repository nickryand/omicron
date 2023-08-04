// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Utilities for poking at data links.

use crate::link::{Link, LinkKind};
use crate::process::{BoxedExecutor, ExecutionError, PFEXEC};
use crate::zone::IPADM;
use omicron_common::api::external::MacAddr;
use omicron_common::vlan::VlanID;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::str::Utf8Error;

pub const VNIC_PREFIX: &str = "ox";
pub const VNIC_PREFIX_CONTROL: &str = "oxControl";
pub const VNIC_PREFIX_BOOTSTRAP: &str = "oxBootstrap";

/// Prefix used to name VNICs over xde devices / OPTE ports.
// TODO-correctness: Remove this when `xde` devices can be directly used beneath
// Viona, and thus plumbed directly to guests.
pub const VNIC_PREFIX_GUEST: &str = "vopte";

/// Path to the DLADM command.
pub const DLADM: &str = "/usr/sbin/dladm";

/// The name of the etherstub to be created for the underlay network.
pub const UNDERLAY_ETHERSTUB_NAME: &str = "underlay_stub0";

/// The name of the etherstub to be created for the bootstrap network.
pub const BOOTSTRAP_ETHERSTUB_NAME: &str = "bootstrap_stub0";

/// The name of the etherstub VNIC to be created in the global zone for the
/// underlay network.
pub const UNDERLAY_ETHERSTUB_VNIC_NAME: &str = "underlay0";

/// The name of the etherstub VNIC to be created in the global zone for the
/// bootstrap network.
pub const BOOTSTRAP_ETHERSTUB_VNIC_NAME: &str = "bootstrap0";

/// The prefix for Chelsio link names.
pub const CHELSIO_LINK_PREFIX: &str = "cxgbe";

/// Errors returned from [`Dladm::find_physical`].
#[derive(thiserror::Error, Debug)]
pub enum FindPhysicalLinkError {
    #[error("Failed to find physical link: {0}")]
    Execution(#[from] ExecutionError),

    #[error("No Physical Link devices found")]
    NoPhysicalLinkFound,

    #[error("Unexpected non-UTF-8 link name")]
    NonUtf8Output(Utf8Error),
}

/// Errors returned from [`Dladm::get_mac`].
#[derive(thiserror::Error, Debug)]
pub enum GetMacError {
    #[error("Mac Address cannot be looked up; Link not found: {0:?}")]
    NotFound(PhysicalLink),

    #[error("Failed to get MAC address: {0}")]
    Execution(#[from] ExecutionError),

    #[error("Failed to parse MAC: {0}")]
    ParseMac(#[from] macaddr::ParseError),
}

/// Errors returned from [`Dladm::create_vnic`].
#[derive(thiserror::Error, Debug)]
#[error("Failed to create VNIC {name} on link {link:?}: {err}")]
pub struct CreateVnicError {
    name: String,
    link: String,
    #[source]
    err: ExecutionError,
}

/// Errors returned from [`Dladm::get_vnics`].
#[derive(thiserror::Error, Debug)]
#[error("Failed to get vnics: {err}")]
pub struct GetVnicError {
    #[source]
    err: ExecutionError,
}

/// Errors returned from [`Dladm::get_simulated_tfports`].
#[derive(thiserror::Error, Debug)]
#[error("Failed to get simnets: {err}")]
pub struct GetSimnetError {
    #[source]
    err: ExecutionError,
}

/// Errors returned from [`Dladm::delete_vnic`].
#[derive(thiserror::Error, Debug)]
#[error("Failed to delete vnic {name}: {err}")]
pub struct DeleteVnicError {
    name: String,
    #[source]
    err: ExecutionError,
}

/// Errors returned from [`Dladm::get_linkprop`].
#[derive(thiserror::Error, Debug)]
#[error(
    "Failed to get link property \"{prop_name}\" on vnic {link_name}: {err}"
)]
pub struct GetLinkpropError {
    link_name: String,
    prop_name: String,
    #[source]
    err: ExecutionError,
}

/// Errors returned from [`Dladm::set_linkprop`].
#[derive(thiserror::Error, Debug)]
#[error("Failed to set link property \"{prop_name}\" to \"{prop_value}\" on vnic {link_name}: {err}")]
pub struct SetLinkpropError {
    link_name: String,
    prop_name: String,
    prop_value: String,
    #[source]
    err: ExecutionError,
}

/// Errors returned from [`Dladm::reset_linkprop`].
#[derive(thiserror::Error, Debug)]
#[error(
    "Failed to reset link property \"{prop_name}\" on vnic {link_name}: {err}"
)]
pub struct ResetLinkpropError {
    link_name: String,
    prop_name: String,
    #[source]
    err: ExecutionError,
}

/// The name of a physical datalink.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PhysicalLink(pub String);

impl ToString for PhysicalLink {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

/// The name of an etherstub
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Etherstub(pub String);

/// The name of an etherstub's underlay VNIC
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct EtherstubVnic(pub String);

/// Identifies that an object may be used to create a VNIC.
pub trait VnicSource {
    fn name(&self) -> &str;
}

impl VnicSource for Etherstub {
    fn name(&self) -> &str {
        &self.0
    }
}

impl VnicSource for PhysicalLink {
    fn name(&self) -> &str {
        &self.0
    }
}

/// Wraps commands for interacting with data links.
pub struct Dladm {}

impl Dladm {
    /// Creates an etherstub, or returns one which already exists.
    pub fn ensure_etherstub(
        executor: &BoxedExecutor,
        name: &str,
    ) -> Result<Etherstub, ExecutionError> {
        if let Ok(stub) = Self::get_etherstub(executor, name) {
            return Ok(stub);
        }
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "create-etherstub", "-t", name]);
        executor.execute(cmd)?;
        Ok(Etherstub(name.to_string()))
    }

    /// Finds an etherstub.
    fn get_etherstub(
        executor: &BoxedExecutor,
        name: &str,
    ) -> Result<Etherstub, ExecutionError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "show-etherstub", name]);
        executor.execute(cmd)?;
        Ok(Etherstub(name.to_string()))
    }

    /// Creates a VNIC on top of the etherstub.
    ///
    /// This VNIC is not tracked like [`crate::link::Link`], because
    /// it is expected to exist for the lifetime of the sled.
    pub fn ensure_etherstub_vnic(
        executor: &BoxedExecutor,
        source: &Etherstub,
    ) -> Result<EtherstubVnic, CreateVnicError> {
        let (vnic_name, mtu) = match source.0.as_str() {
            UNDERLAY_ETHERSTUB_NAME => (UNDERLAY_ETHERSTUB_VNIC_NAME, 9000),
            BOOTSTRAP_ETHERSTUB_NAME => (BOOTSTRAP_ETHERSTUB_VNIC_NAME, 1500),
            _ => unreachable!(),
        };
        if let Ok(vnic) = Self::get_etherstub_vnic(executor, vnic_name) {
            return Ok(vnic);
        }
        Self::create_vnic(executor, source, vnic_name, None, None, mtu)?;
        Ok(EtherstubVnic(vnic_name.to_string()))
    }

    fn get_etherstub_vnic(
        executor: &BoxedExecutor,
        name: &str,
    ) -> Result<EtherstubVnic, ExecutionError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "show-vnic", name]);
        executor.execute(cmd)?;
        Ok(EtherstubVnic(name.to_string()))
    }

    // Return the name of the IP interface over the etherstub VNIC, if it
    // exists.
    fn get_etherstub_vnic_interface(
        executor: &BoxedExecutor,
        name: &str,
    ) -> Result<String, ExecutionError> {
        let mut cmd = std::process::Command::new(PFEXEC);
        let cmd = cmd.args(&[IPADM, "show-if", "-p", "-o", "IFNAME", name]);
        executor.execute(cmd)?;
        Ok(name.to_string())
    }

    /// Delete the VNIC over the inter-zone comms etherstub.
    pub fn delete_etherstub_vnic(
        executor: &BoxedExecutor,
        name: &str,
    ) -> Result<(), ExecutionError> {
        // It's not clear why, but this requires deleting the _interface_ that's
        // over the VNIC first. Other VNICs don't require this for some reason.
        if Self::get_etherstub_vnic_interface(executor, name).is_ok() {
            let mut cmd = std::process::Command::new(PFEXEC);
            let cmd = cmd.args(&[IPADM, "delete-if", name]);
            executor.execute(cmd)?;
        }

        if Self::get_etherstub_vnic(executor, name).is_ok() {
            let mut cmd = std::process::Command::new(PFEXEC);
            let cmd = cmd.args(&[DLADM, "delete-vnic", name]);
            executor.execute(cmd)?;
        }
        Ok(())
    }

    /// Delete the inter-zone comms etherstub.
    pub fn delete_etherstub(
        executor: &BoxedExecutor,
        name: &str,
    ) -> Result<(), ExecutionError> {
        if Self::get_etherstub(executor, name).is_ok() {
            let mut cmd = std::process::Command::new(PFEXEC);
            let cmd = cmd.args(&[DLADM, "delete-etherstub", name]);
            executor.execute(cmd)?;
        }
        Ok(())
    }

    /// Verify that the given link exists
    pub fn verify_link(
        executor: &BoxedExecutor,
        link: &str,
    ) -> Result<Link, FindPhysicalLinkError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "show-link", "-p", "-o", "LINK", link]);
        let output = executor.execute(cmd)?;
        match String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(|s| s.trim())
        {
            Some(x) if x == link => Ok(Link::wrap_physical(executor, link)),
            _ => Err(FindPhysicalLinkError::NoPhysicalLinkFound),
        }
    }

    /// Returns the name of the first observed physical data link.
    pub fn find_physical(
        executor: &BoxedExecutor,
    ) -> Result<PhysicalLink, FindPhysicalLinkError> {
        // TODO: This is arbitrary, but we're currently grabbing the first
        // physical device. Should we have a more sophisticated method for
        // selection?
        Self::list_physical(executor)?
            .into_iter()
            .next()
            .ok_or_else(|| FindPhysicalLinkError::NoPhysicalLinkFound)
    }

    /// List the extant physical data links on the system.
    ///
    /// Note that this returns _all_ links.
    pub fn list_physical(
        executor: &BoxedExecutor,
    ) -> Result<Vec<PhysicalLink>, FindPhysicalLinkError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "show-phys", "-p", "-o", "LINK"]);
        let output = executor.execute(cmd)?;
        std::str::from_utf8(&output.stdout)
            .map_err(FindPhysicalLinkError::NonUtf8Output)
            .map(|stdout| {
                stdout
                    .lines()
                    .map(|name| PhysicalLink(name.trim().to_string()))
                    .collect()
            })
    }

    /// Returns the MAC address of a physical link.
    pub fn get_mac(
        executor: &BoxedExecutor,
        link: &PhysicalLink,
    ) -> Result<MacAddr, GetMacError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[
            DLADM,
            "show-phys",
            "-m",
            "-p",
            "-o",
            "ADDRESS",
            &link.0,
        ]);
        let output = executor.execute(cmd)?;
        let name = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(|s| s.trim())
            .ok_or_else(|| GetMacError::NotFound(link.clone()))?
            .to_string();

        // Ensure the MAC address is zero-padded, so it may be parsed as a
        // MacAddr. This converts segments like ":a" to ":0a".
        let name = name
            .split(':')
            .map(|segment| format!("{:0>2}", segment))
            .collect::<Vec<String>>()
            .join(":");
        let mac = MacAddr::from_str(&name)?;
        Ok(mac)
    }

    /// Creates a new VNIC atop a physical device.
    ///
    /// * `physical`: The physical link on top of which a device will be
    /// created.
    /// * `vnic_name`: Exact name of the VNIC to be created.
    /// * `mac`: An optional unicast MAC address for the newly created NIC.
    /// * `vlan`: An optional VLAN ID for VLAN tagging.
    pub fn create_vnic<T: VnicSource + 'static>(
        executor: &BoxedExecutor,
        source: &T,
        vnic_name: &str,
        mac: Option<MacAddr>,
        vlan: Option<VlanID>,
        mtu: usize,
    ) -> Result<(), CreateVnicError> {
        let mut command = std::process::Command::new(PFEXEC);
        let mut args = vec![
            DLADM.to_string(),
            "create-vnic".to_string(),
            "-t".to_string(),
            "-l".to_string(),
            source.name().to_string(),
        ];

        if let Some(mac) = mac {
            args.push("-m".to_string());
            args.push(mac.0.to_string());
        }

        if let Some(vlan) = vlan {
            args.push("-v".to_string());
            args.push(vlan.to_string());
        }

        args.push("-p".to_string());
        args.push(format!("mtu={mtu}"));

        args.push(vnic_name.to_string());

        let cmd = command.args(&args);
        executor.execute(cmd).map_err(|err| CreateVnicError {
            name: vnic_name.to_string(),
            link: source.name().to_string(),
            err,
        })?;

        // In certain situations, `create-vnic -p mtu=N` does not actually set
        // the mtu to N. Set it here using `set-linkprop`.
        //
        // See https://www.illumos.org/issues/15695 for the illumos bug.
        let mut command = std::process::Command::new(PFEXEC);
        let prop = format!("mtu={}", mtu);
        let cmd = command.args(&[
            DLADM,
            "set-linkprop",
            "-t",
            "-p",
            &prop,
            vnic_name,
        ]);
        executor.execute(cmd).map_err(|err| CreateVnicError {
            name: vnic_name.to_string(),
            link: source.name().to_string(),
            err,
        })?;

        Ok(())
    }

    /// Returns VNICs that may be managed by the Sled Agent.
    pub fn get_vnics(
        executor: &BoxedExecutor,
    ) -> Result<Vec<String>, GetVnicError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "show-vnic", "-p", "-o", "LINK"]);
        let output =
            executor.execute(cmd).map_err(|err| GetVnicError { err })?;

        let vnics = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|name| {
                // Ensure this is a kind of VNIC that the sled agent could be
                // responsible for.
                match LinkKind::from_name(name) {
                    Some(_) => Some(name.to_owned()),
                    None => None,
                }
            })
            .collect();
        Ok(vnics)
    }

    /// Returns simnet links masquerading as tfport devices
    pub fn get_simulated_tfports(
        executor: &BoxedExecutor,
    ) -> Result<Vec<String>, GetSimnetError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "show-simnet", "-p", "-o", "LINK"]);
        let output =
            executor.execute(cmd).map_err(|err| GetSimnetError { err })?;

        let tfports = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|name| {
                if name.starts_with("tfport") {
                    Some(name.to_owned())
                } else {
                    None
                }
            })
            .collect();
        Ok(tfports)
    }

    /// Remove a vnic from the sled.
    pub fn delete_vnic(
        executor: &BoxedExecutor,
        name: &str,
    ) -> Result<(), DeleteVnicError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[DLADM, "delete-vnic", name]);
        executor
            .execute(cmd)
            .map_err(|err| DeleteVnicError { name: name.to_string(), err })?;
        Ok(())
    }

    /// Get a link property value on a VNIC
    pub fn get_linkprop(
        executor: &BoxedExecutor,
        vnic: &str,
        prop_name: &str,
    ) -> Result<String, GetLinkpropError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[
            DLADM,
            "show-linkprop",
            "-c",
            "-o",
            "value",
            "-p",
            prop_name,
            vnic,
        ]);
        let result = executor.execute(cmd).map_err(|err| GetLinkpropError {
            link_name: vnic.to_string(),
            prop_name: prop_name.to_string(),
            err,
        })?;
        Ok(String::from_utf8_lossy(&result.stdout).into_owned())
    }
    /// Set a link property on a VNIC
    pub fn set_linkprop(
        executor: &BoxedExecutor,
        vnic: &str,
        prop_name: &str,
        prop_value: &str,
    ) -> Result<(), SetLinkpropError> {
        let mut command = std::process::Command::new(PFEXEC);
        let prop = format!("{}={}", prop_name, prop_value);
        let cmd =
            command.args(&[DLADM, "set-linkprop", "-t", "-p", &prop, vnic]);
        executor.execute(cmd).map_err(|err| SetLinkpropError {
            link_name: vnic.to_string(),
            prop_name: prop_name.to_string(),
            prop_value: prop_value.to_string(),
            err,
        })?;
        Ok(())
    }

    /// Reset a link property on a VNIC
    pub fn reset_linkprop(
        executor: &BoxedExecutor,
        vnic: &str,
        prop_name: &str,
    ) -> Result<(), ResetLinkpropError> {
        let mut command = std::process::Command::new(PFEXEC);
        let cmd = command.args(&[
            DLADM,
            "reset-linkprop",
            "-t",
            "-p",
            prop_name,
            vnic,
        ]);
        executor.execute(cmd).map_err(|err| ResetLinkpropError {
            link_name: vnic.to_string(),
            prop_name: prop_name.to_string(),
            err,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::process::{FakeExecutor, Input, OutputExt, StaticHandler};
    use omicron_test_utils::dev;
    use std::process::Output;

    #[test]
    fn ensure_new_etherstub() {
        let logctx = dev::test_setup_log("ensure_new_etherstub");

        let mut handler = StaticHandler::new();
        handler.expect_fail(format!("{PFEXEC} {DLADM} show-etherstub mystub1"));
        handler
            .expect_ok(format!("{PFEXEC} {DLADM} create-etherstub -t mystub1"));

        let executor = FakeExecutor::new(logctx.log.clone());
        executor.set_static_handler(handler);

        let etherstub =
            Dladm::ensure_etherstub(&executor.as_executor(), "mystub1")
                .expect("Failed to ensure etherstub");
        assert_eq!(etherstub.0, "mystub1");

        logctx.cleanup_successful();
    }

    #[test]
    fn ensure_existing_etherstub() {
        let logctx = dev::test_setup_log("ensure_existing_etherstub");

        let mut handler = StaticHandler::new();
        handler.expect_ok(format!("{PFEXEC} {DLADM} show-etherstub mystub1"));
        let executor = FakeExecutor::new(logctx.log.clone());
        executor.set_static_handler(handler);

        let etherstub =
            Dladm::ensure_etherstub(&executor.as_executor(), "mystub1")
                .expect("Failed to ensure etherstub");
        assert_eq!(etherstub.0, "mystub1");

        logctx.cleanup_successful();
    }

    #[test]
    fn ensure_existing_etherstub_vnic() {
        let logctx = dev::test_setup_log("ensure_existing_etherstub_vnic");

        let mut handler = StaticHandler::new();
        handler.expect_ok(format!(
            "{PFEXEC} {DLADM} show-etherstub {UNDERLAY_ETHERSTUB_NAME}"
        ));
        handler.expect_ok(format!(
            "{PFEXEC} {DLADM} show-vnic {UNDERLAY_ETHERSTUB_VNIC_NAME}"
        ));
        let executor = FakeExecutor::new(logctx.log.clone());
        executor.set_static_handler(handler);

        let executor = &executor.as_executor();
        let etherstub =
            Dladm::ensure_etherstub(executor, UNDERLAY_ETHERSTUB_NAME)
                .expect("Failed to ensure etherstub");
        let _vnic = Dladm::ensure_etherstub_vnic(executor, &etherstub)
            .expect("Failed to ensure etherstub VNIC");

        logctx.cleanup_successful();
    }

    #[test]
    fn ensure_new_etherstub_vnic() {
        let logctx = dev::test_setup_log("ensure_new_etherstub_vnic");

        let mut handler = StaticHandler::new();
        handler.expect_ok(format!(
            "{PFEXEC} {DLADM} show-etherstub {UNDERLAY_ETHERSTUB_NAME}"
        ));
        handler.expect_fail(format!(
            "{PFEXEC} {DLADM} show-vnic {UNDERLAY_ETHERSTUB_VNIC_NAME}"
        ));
        handler.expect_ok(format!(
            "{PFEXEC} {DLADM} create-vnic -t -l {UNDERLAY_ETHERSTUB_NAME} \
            -p mtu=9000 {UNDERLAY_ETHERSTUB_VNIC_NAME}"
        ));
        handler.expect_ok(format!(
            "{PFEXEC} {DLADM} set-linkprop -t -p mtu=9000 \
            {UNDERLAY_ETHERSTUB_VNIC_NAME}"
        ));
        let executor = FakeExecutor::new(logctx.log.clone());
        executor.set_static_handler(handler);

        let executor = &executor.as_executor();
        let etherstub =
            Dladm::ensure_etherstub(executor, UNDERLAY_ETHERSTUB_NAME)
                .expect("Failed to ensure etherstub");
        let _vnic = Dladm::ensure_etherstub_vnic(executor, &etherstub)
            .expect("Failed to ensure etherstub VNIC");

        logctx.cleanup_successful();
    }

    #[test]
    fn only_parse_oxide_vnics() {
        let logctx = dev::test_setup_log("only_parse_oxide_vnics");

        let mut handler = StaticHandler::new();
        handler.expect(
            Input::shell(format!("{PFEXEC} {DLADM} show-vnic -p -o LINK")),
            Output::success().set_stdout(
                "oxVnic\nvopteVnic\nInvalid\noxBootstrapVnic\nInvalid",
            ),
        );
        let executor = FakeExecutor::new(logctx.log.clone());
        executor.set_static_handler(handler);

        let executor = &executor.as_executor();
        let vnics = Dladm::get_vnics(executor).expect("Failed to get VNICs");

        assert_eq!(vnics[0], "oxVnic");
        assert_eq!(vnics[1], "vopteVnic");
        assert_eq!(vnics[2], "oxBootstrapVnic");
        assert_eq!(vnics.len(), 3);

        logctx.cleanup_successful();
    }
}
