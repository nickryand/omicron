// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! illumos-specific mechanisms for parsing disk info.

use crate::hardware::{DiskError, DiskPaths, DiskVariant, Partition};
use crate::illumos::zpool::{Zpool, ZpoolName};
use slog::Logger;
use std::path::PathBuf;
use uuid::Uuid;

// The expected layout of an M.2 device within the Oxide rack.
//
// Partitions beyond this "expected partition" array are ignored.
const M2_EXPECTED_PARTITION_COUNT: usize = 6;
static M2_EXPECTED_PARTITIONS: [Partition; M2_EXPECTED_PARTITION_COUNT] = [
    Partition::BootImage,
    Partition::Reserved,
    Partition::Reserved,
    Partition::Reserved,
    Partition::DumpDevice,
    Partition::ZfsPool,
];

// The expected layout of a U.2 device within the Oxide rack.
//
// Partitions beyond this "expected partition" array are ignored.
const U2_EXPECTED_PARTITION_COUNT: usize = 1;
static U2_EXPECTED_PARTITIONS: [Partition; U2_EXPECTED_PARTITION_COUNT] =
    [Partition::ZfsPool];

fn parse_partition_types<const N: usize>(
    path: &PathBuf,
    partitions: &Vec<libefi_illumos::Partition>,
    expected_partitions: &[Partition; N],
) -> Result<Vec<Partition>, DiskError> {
    if partitions.len() != N {
        return Err(DiskError::BadPartitionLayout {
            path: path.clone(),
            why: format!(
                "Expected {} partitions, only saw {}",
                partitions.len(),
                N
            ),
        });
    }
    for i in 0..N {
        if partitions[i].index() != i {
            return Err(DiskError::BadPartitionLayout {
                path: path.clone(),
                why: format!(
                    "The {i}-th partition has index {}",
                    partitions[i].index()
                ),
            });
        }

        // NOTE: If we wanted to, we could validate additional information about
        // the size, GUID, or name of the partition. At the moment, however,
        // we're relying on the index within the partition table to indicate the
        // "intent" of the partition.
    }

    Ok(expected_partitions.iter().map(|p| p.clone()).collect())
}

/// Parses, validates, and ensures the partition layout within a disk.
///
/// Returns a Vec of partitions on success. The index of the Vec is guaranteed
/// to also be the index of the partition.
pub fn ensure_partition_layout(
    log: &Logger,
    paths: &DiskPaths,
    variant: DiskVariant,
) -> Result<Vec<Partition>, DiskError> {
    // Open the "Whole Disk" as a raw device to be parsed by the
    // libefi-illumos library. This lets us peek at the GPT before
    // making too many assumptions about it.
    let raw = true;
    let path = paths.whole_disk(raw);

    let gpt = match libefi_illumos::Gpt::read(&path) {
        Ok(gpt) => {
            // This should be the common steady-state case
            info!(
                log,
                "Disk at {} already has a GPT",
                paths.devfs_path.display()
            );
            gpt
        }
        Err(libefi_illumos::Error::LabelNotFound) => {
            // Fresh U.2 disks are an example of devices where "we don't expect
            // a GPT to exist".
            info!(
                log,
                "Disk at {} does not have a GPT",
                paths.devfs_path.display()
            );

            // For ZFS-implementation-specific reasons, Zpool create can only
            // act on devices under the "/dev" hierarchy, rather than the device
            // path which exists in the "/devices" directory.
            let dev_path = if let Some(dev_path) = &paths.dev_path {
                dev_path
            } else {
                return Err(DiskError::CannotFormatMissingDevPath { path });
            };
            match variant {
                DiskVariant::U2 => {
                    info!(
                        log,
                        "Formatting zpool on disk {}",
                        paths.devfs_path.display()
                    );
                    // If a zpool does not already exist, create one.
                    let zpool_name = ZpoolName::new(Uuid::new_v4());
                    Zpool::create(zpool_name, dev_path)?;
                    return Ok(vec![Partition::ZfsPool]);
                }
                DiskVariant::M2 => {
                    // TODO: If we see a completely empty M.2, should we create
                    // the expected partitions? Or would it be wiser to infer
                    // that this indicates an unexpected error conditions that
                    // needs mitigation?
                    todo!("Provisioning M.2 devices not yet supported");
                }
            }
        }
        Err(err) => {
            return Err(DiskError::Gpt {
                path,
                error: anyhow::Error::new(err),
            });
        }
    };
    let mut partitions: Vec<_> = gpt.partitions().collect();
    match variant {
        DiskVariant::U2 => {
            partitions.truncate(U2_EXPECTED_PARTITION_COUNT);
            parse_partition_types(&path, &partitions, &U2_EXPECTED_PARTITIONS)
        }
        DiskVariant::M2 => {
            partitions.truncate(M2_EXPECTED_PARTITION_COUNT);
            parse_partition_types(&path, &partitions, &M2_EXPECTED_PARTITIONS)
        }
    }
}
