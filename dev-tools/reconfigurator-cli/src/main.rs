// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! developer REPL for driving blueprint planning

use anyhow::{anyhow, bail, Context};
use camino::Utf8PathBuf;
use clap::CommandFactory;
use clap::FromArgMatches;
use clap::ValueEnum;
use clap::{Args, Parser, Subcommand};
use dns_service_client::DnsDiff;
use indexmap::IndexMap;
use nexus_reconfigurator_execution::blueprint_external_dns_config;
use nexus_reconfigurator_execution::blueprint_internal_dns_config;
use nexus_reconfigurator_planning::blueprint_builder::BlueprintBuilder;
use nexus_reconfigurator_planning::planner::Planner;
use nexus_reconfigurator_planning::system::{
    SledBuilder, SledHwInventory, SystemDescription,
};
use nexus_types::deployment::{Blueprint, UnstableReconfiguratorState};
use nexus_types::internal_api::params::DnsConfigParams;
use nexus_types::inventory::Collection;
use nexus_types::inventory::OmicronZonesConfig;
use nexus_types::inventory::SledRole;
use omicron_common::api::external::Generation;
use omicron_common::api::external::Name;
use reedline::{Reedline, Signal};
use std::collections::BTreeMap;
use std::io::BufRead;
use swrite::{swriteln, SWrite};
use tabled::Tabled;
use uuid::Uuid;

/// REPL state
#[derive(Debug)]
struct ReconfiguratorSim {
    /// describes the sleds in the system
    ///
    /// This resembles what we get from the `sled` table in a real system.  It
    /// also contains enough information to generate inventory collections that
    /// describe the system.
    system: SystemDescription,

    /// inventory collections created by the user
    collections: IndexMap<Uuid, Collection>,

    /// blueprints created by the user
    blueprints: IndexMap<Uuid, Blueprint>,

    /// internal DNS configurations
    internal_dns: BTreeMap<Generation, DnsConfigParams>,
    /// external DNS configurations
    external_dns: BTreeMap<Generation, DnsConfigParams>,

    /// Set of silo names configured
    ///
    /// These are used to determine the contents of external DNS.
    silo_names: Vec<Name>,

    /// External DNS zone name configured
    external_dns_zone_name: String,

    /// Policy overrides
    num_nexus: Option<u16>,

    log: slog::Logger,
}

/// interactive REPL for exploring the planner
#[derive(Parser, Debug)]
struct CmdReconfiguratorSim {
    input_file: Option<Utf8PathBuf>,
}

// REPL implementation

fn main() -> anyhow::Result<()> {
    let cmd = CmdReconfiguratorSim::parse();

    let log = dropshot::ConfigLogging::StderrTerminal {
        level: dropshot::ConfigLoggingLevel::Debug,
    }
    .to_logger("reconfigurator-sim")
    .context("creating logger")?;

    let mut sim = ReconfiguratorSim {
        system: SystemDescription::new(),
        collections: IndexMap::new(),
        blueprints: IndexMap::new(),
        internal_dns: BTreeMap::new(),
        external_dns: BTreeMap::new(),
        log,
        silo_names: vec!["example-silo".parse().unwrap()],
        external_dns_zone_name: String::from("oxide.example"),
        num_nexus: None,
    };

    if let Some(input_file) = cmd.input_file {
        let file = std::fs::File::open(&input_file)
            .with_context(|| format!("open {:?}", &input_file))?;
        let bufread = std::io::BufReader::new(file);
        for maybe_buffer in bufread.lines() {
            let buffer = maybe_buffer
                .with_context(|| format!("read {:?}", &input_file))?;
            println!("> {}", buffer);
            match process_entry(&mut sim, buffer) {
                LoopResult::Continue => (),
                LoopResult::Bail(error) => return Err(error),
            }
            println!("");
        }
    } else {
        let mut ed = Reedline::create();
        let prompt = reedline::DefaultPrompt::new(
            reedline::DefaultPromptSegment::Empty,
            reedline::DefaultPromptSegment::Empty,
        );
        loop {
            match ed.read_line(&prompt) {
                Ok(Signal::Success(buffer)) => {
                    match process_entry(&mut sim, buffer) {
                        LoopResult::Continue => (),
                        LoopResult::Bail(error) => return Err(error),
                    }
                }
                Ok(Signal::CtrlD) | Ok(Signal::CtrlC) => break,
                Err(error) => {
                    bail!("reconfigurator-cli: unexpected error: {:#}", error);
                }
            }
        }
    }

    Ok(())
}

/// Describes next steps after evaluating one "line" of user input
///
/// This could just be `Result`, but it's easy to misuse that here because
/// _commands_ might fail all the time without needing to bail out of the REPL.
/// We use a separate type for clarity about what success/failure actually
/// means.
enum LoopResult {
    /// Show the prompt and accept another command
    Continue,

    /// Exit the REPL with a fatal error
    Bail(anyhow::Error),
}

/// Processes one "line" of user input.
fn process_entry(sim: &mut ReconfiguratorSim, entry: String) -> LoopResult {
    // If no input was provided, take another lap (print the prompt and accept
    // another line).  This gets handled specially because otherwise clap would
    // treat this as a usage error and print a help message, which isn't what we
    // want here.
    if entry.trim().is_empty() {
        return LoopResult::Continue;
    }

    // Parse the line of input as a REPL command.
    //
    // Using `split_whitespace()` like this is going to be a problem if we ever
    // want to support arguments with whitespace in them (using quotes).  But
    // it's good enough for now.
    let parts = entry.split_whitespace();
    let parsed_command = TopLevelArgs::command()
        .multicall(true)
        .try_get_matches_from(parts)
        .and_then(|matches| TopLevelArgs::from_arg_matches(&matches));
    let command = match parsed_command {
        Err(error) => {
            // We failed to parse the command.  Print the error.
            return match error.print() {
                // Assuming that worked, just take another lap.
                Ok(_) => LoopResult::Continue,
                // If we failed to even print the error, that itself is a fatal
                // error.
                Err(error) => LoopResult::Bail(
                    anyhow!(error).context("printing previous error"),
                ),
            };
        }
        Ok(TopLevelArgs { command }) => command,
    };

    // Dispatch to the command's handler.
    let cmd_result = match command {
        Commands::SledList => cmd_sled_list(sim),
        Commands::SledAdd(args) => cmd_sled_add(sim, args),
        Commands::SledShow(args) => cmd_sled_show(sim, args),
        Commands::SiloList => cmd_silo_list(sim),
        Commands::SiloAdd(args) => cmd_silo_add(sim, args),
        Commands::SiloRemove(args) => cmd_silo_remove(sim, args),
        Commands::InventoryList => cmd_inventory_list(sim),
        Commands::InventoryGenerate => cmd_inventory_generate(sim),
        Commands::BlueprintList => cmd_blueprint_list(sim),
        Commands::BlueprintFromInventory(args) => {
            cmd_blueprint_from_inventory(sim, args)
        }
        Commands::BlueprintPlan(args) => cmd_blueprint_plan(sim, args),
        Commands::BlueprintShow(args) => cmd_blueprint_show(sim, args),
        Commands::BlueprintDiff(args) => cmd_blueprint_diff(sim, args),
        Commands::BlueprintDiffDns(args) => cmd_blueprint_diff_dns(sim, args),
        Commands::BlueprintDiffInventory(args) => {
            cmd_blueprint_diff_inventory(sim, args)
        }
        Commands::Show => cmd_show(sim),
        Commands::Set(args) => cmd_set(sim, args),
        Commands::Load(args) => cmd_load(sim, args),
        Commands::FileContents(args) => cmd_file_contents(args),
        Commands::Save(args) => cmd_save(sim, args),
    };

    match cmd_result {
        Err(error) => println!("error: {:#}", error),
        Ok(Some(s)) => println!("{}", s),
        Ok(None) => (),
    }

    LoopResult::Continue
}

// clap configuration for the REPL commands

/// reconfigurator-sim: simulate blueprint planning and execution
#[derive(Debug, Parser)]
struct TopLevelArgs {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// list sleds
    SledList,
    /// add a new sled
    SledAdd(SledAddArgs),
    /// show details about one sled
    SledShow(SledArgs),

    /// list silos
    SiloList,
    /// add a silo
    SiloAdd(SiloAddRemoveArgs),
    /// remove a silo
    SiloRemove(SiloAddRemoveArgs),

    /// list all inventory collections
    InventoryList,
    /// generates an inventory collection from the configured sleds
    InventoryGenerate,

    /// list all blueprints
    BlueprintList,
    /// generate a blueprint that represents the contents of an inventory
    BlueprintFromInventory(InventoryArgs),
    /// run planner to generate a new blueprint
    BlueprintPlan(BlueprintPlanArgs),
    /// show details about a blueprint
    BlueprintShow(BlueprintArgs),
    /// show differences between two blueprints
    BlueprintDiff(BlueprintDiffArgs),
    /// show differences between a blueprint and a particular DNS version
    BlueprintDiffDns(BlueprintDiffDnsArgs),
    /// show differences between a blueprint and an inventory collection
    BlueprintDiffInventory(BlueprintDiffInventoryArgs),

    /// show system properties
    Show,
    /// set system properties
    #[command(subcommand)]
    Set(SetArgs),

    /// save state to a file
    Save(SaveArgs),
    /// load state from a file
    Load(LoadArgs),
    /// show information about what's in a saved file
    FileContents(FileContentsArgs),
}

#[derive(Debug, Args)]
struct SledAddArgs {
    /// id of the new sled
    sled_id: Option<Uuid>,
}

#[derive(Debug, Args)]
struct SledArgs {
    /// id of the sled
    sled_id: Uuid,
}

#[derive(Debug, Args)]
struct SiloAddRemoveArgs {
    /// name of the silo
    silo_name: Name,
}

#[derive(Debug, Args)]
struct InventoryArgs {
    /// id of the inventory collection to use in planning
    collection_id: Uuid,
}

#[derive(Debug, Args)]
struct BlueprintPlanArgs {
    /// id of the blueprint on which this one will be based
    parent_blueprint_id: Uuid,
    /// id of the inventory collection to use in planning
    collection_id: Uuid,
}

#[derive(Debug, Args)]
struct BlueprintArgs {
    /// id of the blueprint
    blueprint_id: Uuid,
}

#[derive(Debug, Args)]
struct BlueprintDiffDnsArgs {
    /// DNS group (internal or external)
    dns_group: CliDnsGroup,
    /// DNS version to diff against
    dns_version: u32,
    /// id of the blueprint
    blueprint_id: Uuid,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliDnsGroup {
    Internal,
    External,
}

#[derive(Debug, Args)]
struct BlueprintDiffInventoryArgs {
    /// id of the inventory collection
    collection_id: Uuid,
    /// id of the blueprint
    blueprint_id: Uuid,
}

#[derive(Debug, Args)]
struct BlueprintDiffArgs {
    /// id of the first blueprint
    blueprint1_id: Uuid,
    /// id of the second blueprint
    blueprint2_id: Uuid,
}

#[derive(Debug, Subcommand)]
enum SetArgs {
    /// target number of Nexus instances (for planning)
    NumNexus { num_nexus: u16 },
    /// system's external DNS zone name (suffix)
    ExternalDnsZoneName { zone_name: String },
}

#[derive(Debug, Args)]
struct LoadArgs {
    /// input file
    filename: Utf8PathBuf,

    /// id of inventory collection to use for sled details
    /// (may be omitted only if the file contains only one collection)
    collection_id: Option<Uuid>,
}

#[derive(Debug, Args)]
struct FileContentsArgs {
    /// input file
    filename: Utf8PathBuf,
}

#[derive(Debug, Args)]
struct SaveArgs {
    /// output file
    filename: Utf8PathBuf,
}

// Command handlers

fn cmd_silo_list(
    sim: &mut ReconfiguratorSim,
) -> anyhow::Result<Option<String>> {
    let mut s = String::new();
    for silo_name in &sim.silo_names {
        swriteln!(s, "{}", silo_name);
    }
    Ok(Some(s))
}

fn cmd_silo_add(
    sim: &mut ReconfiguratorSim,
    args: SiloAddRemoveArgs,
) -> anyhow::Result<Option<String>> {
    if sim.silo_names.contains(&args.silo_name) {
        bail!("silo already exists: {:?}", &args.silo_name);
    }

    sim.silo_names.push(args.silo_name);
    Ok(None)
}

fn cmd_silo_remove(
    sim: &mut ReconfiguratorSim,
    args: SiloAddRemoveArgs,
) -> anyhow::Result<Option<String>> {
    let size_before = sim.silo_names.len();
    sim.silo_names.retain(|n| *n != args.silo_name);
    if sim.silo_names.len() == size_before {
        bail!("no such silo: {:?}", &args.silo_name);
    }
    Ok(None)
}

fn cmd_sled_list(
    sim: &mut ReconfiguratorSim,
) -> anyhow::Result<Option<String>> {
    #[derive(Tabled)]
    #[tabled(rename_all = "SCREAMING_SNAKE_CASE")]
    struct Sled {
        id: Uuid,
        nzpools: usize,
        subnet: String,
    }

    let policy = sim.system.to_policy().context("failed to generate policy")?;
    let rows = policy.sleds.iter().map(|(sled_id, sled_resources)| Sled {
        id: *sled_id,
        subnet: sled_resources.subnet.net().to_string(),
        nzpools: sled_resources.zpools.len(),
    });
    let table = tabled::Table::new(rows)
        .with(tabled::settings::Style::empty())
        .with(tabled::settings::Padding::new(0, 1, 0, 0))
        .to_string();
    Ok(Some(table))
}

fn cmd_sled_add(
    sim: &mut ReconfiguratorSim,
    add: SledAddArgs,
) -> anyhow::Result<Option<String>> {
    let mut new_sled = SledBuilder::new();
    if let Some(sled_id) = add.sled_id {
        new_sled = new_sled.id(sled_id);
    }

    let _ = sim.system.sled(new_sled).context("adding sled")?;
    Ok(Some(String::from("added sled")))
}

fn cmd_sled_show(
    sim: &mut ReconfiguratorSim,
    args: SledArgs,
) -> anyhow::Result<Option<String>> {
    let policy = sim.system.to_policy().context("failed to generate policy")?;
    let sled_id = args.sled_id;
    let sled_resources = policy
        .sleds
        .get(&sled_id)
        .ok_or_else(|| anyhow!("no sled with id {:?}", sled_id))?;
    let mut s = String::new();
    swriteln!(s, "sled {}", sled_id);
    swriteln!(s, "subnet {}", sled_resources.subnet.net());
    swriteln!(s, "zpools ({}):", sled_resources.zpools.len());
    for z in &sled_resources.zpools {
        swriteln!(s, "    {:?}", z);
    }
    Ok(Some(s))
}

fn cmd_inventory_list(
    sim: &mut ReconfiguratorSim,
) -> anyhow::Result<Option<String>> {
    #[derive(Tabled)]
    #[tabled(rename_all = "SCREAMING_SNAKE_CASE")]
    struct InventoryRow {
        id: Uuid,
        nerrors: usize,
        time_done: String,
    }

    let rows = sim.collections.values().map(|collection| {
        let id = collection.id;
        InventoryRow {
            id,
            nerrors: collection.errors.len(),
            time_done: humantime::format_rfc3339_millis(
                collection.time_done.into(),
            )
            .to_string(),
        }
    });
    let table = tabled::Table::new(rows)
        .with(tabled::settings::Style::empty())
        .with(tabled::settings::Padding::new(0, 1, 0, 0))
        .to_string();
    Ok(Some(table))
}

fn cmd_inventory_generate(
    sim: &mut ReconfiguratorSim,
) -> anyhow::Result<Option<String>> {
    let mut builder =
        sim.system.to_collection_builder().context("generating inventory")?;
    // For an inventory we just generated from thin air, pretend like each sled
    // has no zones on it.
    let sled_ids = sim.system.to_policy().unwrap().sleds.into_keys();
    for sled_id in sled_ids {
        builder
            .found_sled_omicron_zones(
                "fake sled agent",
                sled_id,
                OmicronZonesConfig {
                    generation: Generation::new(),
                    zones: vec![],
                },
            )
            .context("recording Omicron zones")?;
    }
    let inventory = builder.build();
    let rv = format!(
        "generated inventory collection {} from configured sleds",
        inventory.id
    );
    sim.collections.insert(inventory.id, inventory);
    Ok(Some(rv))
}

fn cmd_blueprint_list(
    sim: &mut ReconfiguratorSim,
) -> anyhow::Result<Option<String>> {
    #[derive(Tabled)]
    #[tabled(rename_all = "SCREAMING_SNAKE_CASE")]
    struct BlueprintRow {
        id: Uuid,
    }

    let rows = sim
        .blueprints
        .values()
        .map(|blueprint| BlueprintRow { id: blueprint.id });
    let table = tabled::Table::new(rows)
        .with(tabled::settings::Style::empty())
        .with(tabled::settings::Padding::new(0, 1, 0, 0))
        .to_string();
    Ok(Some(table))
}

fn cmd_blueprint_from_inventory(
    sim: &mut ReconfiguratorSim,
    args: InventoryArgs,
) -> anyhow::Result<Option<String>> {
    let collection_id = args.collection_id;
    let collection = sim
        .collections
        .get(&collection_id)
        .ok_or_else(|| anyhow!("no such collection: {}", collection_id))?;
    let dns_version = Generation::new();
    let policy = sim.system.to_policy().context("generating policy")?;
    let creator = "reconfigurator-sim";
    let blueprint = BlueprintBuilder::build_initial_from_collection(
        collection,
        dns_version,
        dns_version,
        &policy,
        creator,
    )
    .context("building collection")?;
    let rv = format!(
        "generated blueprint {} from inventory collection {}",
        blueprint.id, collection_id
    );
    sim.blueprints.insert(blueprint.id, blueprint);
    Ok(Some(rv))
}

fn cmd_blueprint_plan(
    sim: &mut ReconfiguratorSim,
    args: BlueprintPlanArgs,
) -> anyhow::Result<Option<String>> {
    let parent_blueprint_id = args.parent_blueprint_id;
    let collection_id = args.collection_id;
    let parent_blueprint = sim
        .blueprints
        .get(&parent_blueprint_id)
        .ok_or_else(|| anyhow!("no such blueprint: {}", parent_blueprint_id))?;
    let collection = sim
        .collections
        .get(&collection_id)
        .ok_or_else(|| anyhow!("no such collection: {}", collection_id))?;
    let policy = sim.system.to_policy().context("generating policy")?;
    let creator = "reconfigurator-sim";

    let planner = Planner::new_based_on(
        sim.log.clone(),
        parent_blueprint,
        // The internal and external DNS numbers that go here are supposed to be
        // the _current_ internal and external DNS generations at the point
        // when planning happened.  This is racy (these generations can change
        // immediately after they're fetched from the database) but correctness
        // only requires that the values here be *no newer* than the real
        // values so it's okay if the real values get changed.
        //
        // The problem is we have no real system here to fetch these values
        // from.  What should the value be?
        //
        // - If we assume that the parent blueprint here was successfully
        //   executed immediately before generating this plan, then the values
        //   here should come from the generation number produced by executing
        //   the parent blueprint.
        //
        // - If the parent blueprint was never executed, or execution is still
        //   in progress, or if other blueprints have been executed in the
        //   meantime that changed DNS, then the values here could be different
        //   (older if the blueprint was never executed or is currently
        //   executing and newer if other blueprints have changed DNS in the
        //   meantime).
        //
        // But in this CLI, there's no execution at all.  As a result, there's
        // no way to really choose between these -- and it doesn't really
        // matter, either.  We'll just pick the parent blueprint's.
        parent_blueprint.internal_dns_version,
        parent_blueprint.external_dns_version,
        &policy,
        creator,
        collection,
    )
    .context("creating planner")?;
    let blueprint = planner.plan().context("generating blueprint")?;
    let rv = format!(
        "generated blueprint {} based on parent blueprint {}",
        blueprint.id, parent_blueprint_id,
    );
    sim.blueprints.insert(blueprint.id, blueprint);
    Ok(Some(rv))
}

fn cmd_blueprint_show(
    sim: &mut ReconfiguratorSim,
    args: BlueprintArgs,
) -> anyhow::Result<Option<String>> {
    let blueprint = sim
        .blueprints
        .get(&args.blueprint_id)
        .ok_or_else(|| anyhow!("no such blueprint: {}", args.blueprint_id))?;
    Ok(Some(format!("{}", blueprint.display())))
}

fn cmd_blueprint_diff(
    sim: &mut ReconfiguratorSim,
    args: BlueprintDiffArgs,
) -> anyhow::Result<Option<String>> {
    let mut rv = String::new();
    let blueprint1_id = args.blueprint1_id;
    let blueprint2_id = args.blueprint2_id;
    let blueprint1 = sim
        .blueprints
        .get(&blueprint1_id)
        .ok_or_else(|| anyhow!("no such blueprint: {}", blueprint1_id))?;
    let blueprint2 = sim
        .blueprints
        .get(&blueprint2_id)
        .ok_or_else(|| anyhow!("no such blueprint: {}", blueprint2_id))?;

    let sled_diff = blueprint1.diff_sleds(&blueprint2).display().to_string();
    swriteln!(rv, "{}", sled_diff);

    // Diff'ing DNS is a little trickier.  First, compute what DNS should be for
    // each blueprint.  To do that we need to construct a list of sleds suitable
    // for the executor.
    let sleds_by_id = make_sleds_by_id(&sim)?;
    let internal_dns_config1 = blueprint_internal_dns_config(
        &blueprint1,
        &sleds_by_id,
        &Default::default(),
    )?;
    let internal_dns_config2 = blueprint_internal_dns_config(
        &blueprint2,
        &sleds_by_id,
        &Default::default(),
    )?;
    let dns_diff = DnsDiff::new(&internal_dns_config1, &internal_dns_config2)
        .context("failed to assemble DNS diff")?;
    swriteln!(rv, "internal DNS:\n{}", dns_diff);

    let external_dns_config1 = blueprint_external_dns_config(
        &blueprint1,
        &sim.silo_names,
        sim.external_dns_zone_name.clone(),
    );
    let external_dns_config2 = blueprint_external_dns_config(
        &blueprint2,
        &sim.silo_names,
        sim.external_dns_zone_name.clone(),
    );
    let dns_diff = DnsDiff::new(&external_dns_config1, &external_dns_config2)
        .context("failed to assemble external DNS diff")?;
    swriteln!(rv, "external DNS:\n{}", dns_diff);

    Ok(Some(rv))
}

fn make_sleds_by_id(
    sim: &ReconfiguratorSim,
) -> Result<BTreeMap<Uuid, nexus_reconfigurator_execution::Sled>, anyhow::Error>
{
    let collection = sim
        .system
        .to_collection_builder()
        .context(
            "unexpectedly failed to create collection for current set of sleds",
        )?
        .build();
    let sleds_by_id: BTreeMap<_, _> = collection
        .sled_agents
        .iter()
        .map(|(sled_id, sled_agent_info)| {
            let sled = nexus_reconfigurator_execution::Sled::new(
                *sled_id,
                sled_agent_info.sled_agent_address,
                sled_agent_info.sled_role == SledRole::Scrimlet,
            );
            (*sled_id, sled)
        })
        .collect();
    Ok(sleds_by_id)
}

fn cmd_blueprint_diff_dns(
    sim: &mut ReconfiguratorSim,
    args: BlueprintDiffDnsArgs,
) -> anyhow::Result<Option<String>> {
    let dns_group = args.dns_group;
    let dns_version = Generation::from(args.dns_version);
    let blueprint_id = args.blueprint_id;
    let blueprint = sim
        .blueprints
        .get(&blueprint_id)
        .ok_or_else(|| anyhow!("no such blueprint: {}", blueprint_id))?;

    let existing_dns_config = match dns_group {
        CliDnsGroup::Internal => sim.internal_dns.get(&dns_version),
        CliDnsGroup::External => sim.external_dns.get(&dns_version),
    }
    .ok_or_else(|| {
        anyhow!("no such {:?} DNS version: {}", dns_group, dns_version)
    })?;

    let blueprint_dns_zone = match dns_group {
        CliDnsGroup::Internal => {
            let sleds_by_id = make_sleds_by_id(sim)?;
            blueprint_internal_dns_config(
                &blueprint,
                &sleds_by_id,
                &Default::default(),
            )
            .with_context(|| {
                format!(
                    "computing internal DNS config for blueprint {}",
                    blueprint_id
                )
            })?
        }
        CliDnsGroup::External => blueprint_external_dns_config(
            &blueprint,
            &sim.silo_names,
            sim.external_dns_zone_name.clone(),
        ),
    };

    let existing_dns_zone = existing_dns_config.sole_zone()?;
    let dns_diff = DnsDiff::new(&existing_dns_zone, &blueprint_dns_zone)
        .context("failed to assemble DNS diff")?;
    Ok(Some(dns_diff.to_string()))
}

fn cmd_blueprint_diff_inventory(
    sim: &mut ReconfiguratorSim,
    args: BlueprintDiffInventoryArgs,
) -> anyhow::Result<Option<String>> {
    let collection_id = args.collection_id;
    let blueprint_id = args.blueprint_id;
    let collection = sim.collections.get(&collection_id).ok_or_else(|| {
        anyhow!("no such inventory collection: {}", collection_id)
    })?;
    let blueprint = sim
        .blueprints
        .get(&blueprint_id)
        .ok_or_else(|| anyhow!("no such blueprint: {}", blueprint_id))?;

    let diff = blueprint.diff_sleds_from_collection(&collection);
    Ok(Some(diff.display().to_string()))
}

fn cmd_save(
    sim: &mut ReconfiguratorSim,
    args: SaveArgs,
) -> anyhow::Result<Option<String>> {
    let policy = sim.system.to_policy().context("creating policy")?;
    let saved = UnstableReconfiguratorState {
        policy,
        collections: sim.collections.values().cloned().collect(),
        blueprints: sim.blueprints.values().cloned().collect(),
        internal_dns: sim.internal_dns.clone(),
        external_dns: sim.external_dns.clone(),
        silo_names: sim.silo_names.clone(),
        external_dns_zone_names: vec![sim.external_dns_zone_name.clone()],
    };

    let output_path = &args.filename;
    let outfile = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output_path)
        .with_context(|| format!("open {:?}", output_path))?;
    serde_json::to_writer_pretty(&outfile, &saved)
        .with_context(|| format!("writing to {:?}", output_path))
        .unwrap_or_else(|e| panic!("{:#}", e));
    Ok(Some(format!(
        "saved policy, collections, and blueprints to {:?}",
        output_path
    )))
}

fn cmd_show(sim: &mut ReconfiguratorSim) -> anyhow::Result<Option<String>> {
    let mut s = String::new();
    do_print_properties(&mut s, sim);
    swriteln!(
        s,
        "target number of Nexus instances: {}",
        match sim.num_nexus {
            Some(n) => n.to_string(),
            None => String::from("default"),
        }
    );
    Ok(Some(s))
}

fn do_print_properties(s: &mut String, sim: &ReconfiguratorSim) {
    swriteln!(
        s,
        "configured external DNS zone name: {}",
        sim.external_dns_zone_name,
    );
    swriteln!(
        s,
        "configured silo names: {}",
        sim.silo_names
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    swriteln!(
        s,
        "internal DNS generations: {}",
        sim.internal_dns
            .keys()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );
    swriteln!(
        s,
        "external DNS generations: {}",
        sim.external_dns
            .keys()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );
}

fn cmd_set(
    sim: &mut ReconfiguratorSim,
    args: SetArgs,
) -> anyhow::Result<Option<String>> {
    Ok(Some(match args {
        SetArgs::NumNexus { num_nexus } => {
            let rv = format!("{:?} -> {}", sim.num_nexus, num_nexus);
            sim.num_nexus = Some(num_nexus);
            sim.system.target_nexus_zone_count(usize::from(num_nexus));
            rv
        }
        SetArgs::ExternalDnsZoneName { zone_name } => {
            let rv =
                format!("{:?} -> {:?}", sim.external_dns_zone_name, zone_name);
            sim.external_dns_zone_name = zone_name;
            rv
        }
    }))
}

fn read_file(
    input_path: &camino::Utf8Path,
) -> anyhow::Result<UnstableReconfiguratorState> {
    let file = std::fs::File::open(input_path)
        .with_context(|| format!("open {:?}", input_path))?;
    serde_json::from_reader(file)
        .with_context(|| format!("read {:?}", input_path))
}

fn cmd_load(
    sim: &mut ReconfiguratorSim,
    args: LoadArgs,
) -> anyhow::Result<Option<String>> {
    let input_path = args.filename;
    let collection_id = args.collection_id;
    let loaded = read_file(&input_path)?;

    let mut s = String::new();

    let collection_id = match collection_id {
        Some(s) => s,
        None => match loaded.collections.len() {
            1 => loaded.collections[0].id,
            0 => bail!(
                "no collection_id specified and file contains 0 collections"
            ),
            count => bail!(
                "no collection_id specified and file contains {} \
                    collections: {}",
                count,
                loaded
                    .collections
                    .iter()
                    .map(|c| c.id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
    };

    swriteln!(
        s,
        "using collection {} as source of sled inventory data",
        collection_id
    );
    let primary_collection =
        loaded.collections.iter().find(|c| c.id == collection_id).ok_or_else(
            || {
                anyhow!(
                    "collection {} not found in file {:?}",
                    collection_id,
                    input_path
                )
            },
        )?;

    let current_policy = sim.system.to_policy().context("generating policy")?;
    for (sled_id, sled_resources) in loaded.policy.sleds {
        if current_policy.sleds.contains_key(&sled_id) {
            swriteln!(
                s,
                "sled {}: skipped (one with \
                the same id is already loaded)",
                sled_id
            );
            continue;
        }

        let Some(inventory_sled_agent) =
            primary_collection.sled_agents.get(&sled_id)
        else {
            swriteln!(
                s,
                "error: load sled {}: no inventory found for sled agent in \
                collection {}",
                sled_id,
                collection_id
            );
            continue;
        };

        let inventory_sp = match &inventory_sled_agent.baseboard_id {
            Some(baseboard_id) => {
                let inv_sp = primary_collection
                    .sps
                    .get(baseboard_id)
                    .ok_or_else(|| {
                        anyhow!(
                            "error: load sled {}: missing SP inventory",
                            sled_id
                        )
                    })?;
                let inv_rot = primary_collection
                    .rots
                    .get(baseboard_id)
                    .ok_or_else(|| {
                        anyhow!(
                            "error: load sled {}: missing RoT inventory",
                            sled_id
                        )
                    })?;
                Some(SledHwInventory { baseboard_id, sp: inv_sp, rot: inv_rot })
            }
            None => None,
        };

        let result = sim.system.sled_full(
            sled_id,
            sled_resources,
            inventory_sp,
            inventory_sled_agent,
        );

        match result {
            Ok(_) => swriteln!(s, "sled {} loaded", sled_id),
            Err(error) => {
                swriteln!(s, "error: load sled {}: {:#}", sled_id, error)
            }
        };
    }

    for collection in loaded.collections {
        if sim.collections.contains_key(&collection.id) {
            swriteln!(
                s,
                "collection {}: skipped (one with the \
                same id is already loaded)",
                collection.id
            );
        } else {
            swriteln!(s, "collection {} loaded", collection.id);
            sim.collections.insert(collection.id, collection);
        }
    }

    for blueprint in loaded.blueprints {
        if sim.blueprints.contains_key(&blueprint.id) {
            swriteln!(
                s,
                "blueprint {}: skipped (one with the \
                same id is already loaded)",
                blueprint.id
            );
        } else {
            swriteln!(s, "blueprint {} loaded", blueprint.id);
            sim.blueprints.insert(blueprint.id, blueprint);
        }
    }

    sim.internal_dns = loaded.internal_dns;
    sim.external_dns = loaded.external_dns;
    sim.silo_names = loaded.silo_names;

    let nnames = loaded.external_dns_zone_names.len();
    if nnames > 0 {
        if nnames > 1 {
            swriteln!(
                s,
                "warn: found {} external DNS names; using only the first one",
                nnames
            );
        }
        sim.external_dns_zone_name =
            loaded.external_dns_zone_names.into_iter().next().unwrap();
    }
    do_print_properties(&mut s, sim);

    swriteln!(s, "loaded data from {:?}", input_path);
    Ok(Some(s))
}

fn cmd_file_contents(args: FileContentsArgs) -> anyhow::Result<Option<String>> {
    let loaded = read_file(&args.filename)?;

    let mut s = String::new();

    for (sled_id, sled_resources) in loaded.policy.sleds {
        swriteln!(
            s,
            "sled: {} (subnet: {}, zpools: {})",
            sled_id,
            sled_resources.subnet.net(),
            sled_resources.zpools.len()
        );
    }

    for collection in loaded.collections {
        swriteln!(
            s,
            "collection: {} (errors: {}, completed at: {})",
            collection.id,
            collection.errors.len(),
            humantime::format_rfc3339_millis(collection.time_done.into())
                .to_string(),
        );
    }

    for blueprint in loaded.blueprints {
        swriteln!(
            s,
            "blueprint:  {} (created at: {})",
            blueprint.id,
            blueprint.time_created
        );
    }

    swriteln!(s, "internal DNS generations: {:?}", loaded.internal_dns.keys(),);
    swriteln!(s, "external DNS generations: {:?}", loaded.external_dns.keys(),);
    swriteln!(s, "silo names: {:?}", loaded.silo_names);
    swriteln!(
        s,
        "external DNS zone names: {}",
        loaded.external_dns_zone_names.join(", ")
    );

    Ok(Some(s))
}
