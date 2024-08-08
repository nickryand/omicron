// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::impl_enum_type;
use omicron_common::api::external::Error;
use omicron_common::api::internal;
use serde::{Deserialize, Serialize};

impl_enum_type!(
    #[derive(SqlType, Debug, QueryId)]
    #[diesel(postgres_type(name = "dataset_kind", schema = "public"))]
    pub struct DatasetKindEnum;

    #[derive(Clone, Copy, Debug, AsExpression, FromSqlRow, Serialize, Deserialize, PartialEq)]
    #[diesel(sql_type = DatasetKindEnum)]
    pub enum DatasetKind;

    // Enum values
    Crucible => b"crucible"
    Cockroach => b"cockroach"
    Clickhouse => b"clickhouse"
    ClickhouseKeeper => b"clickhouse_keeper"
    ExternalDns => b"external_dns"
    InternalDns => b"internal_dns"
    ZoneRoot => b"zone_root"
    Zone => b"zone"
    Debug => b"debug"
);

impl DatasetKind {
    pub fn try_into_api(
        self,
        zone_name: Option<String>,
    ) -> Result<internal::shared::DatasetKind, Error> {
        use internal::shared::DatasetKind as ApiKind;
        let k = match (self, zone_name) {
            (Self::Crucible, None) => ApiKind::Crucible,
            (Self::Cockroach, None) => ApiKind::Cockroach,
            (Self::Clickhouse, None) => ApiKind::Clickhouse,
            (Self::ClickhouseKeeper, None) => ApiKind::ClickhouseKeeper,
            (Self::ExternalDns, None) => ApiKind::ExternalDns,
            (Self::InternalDns, None) => ApiKind::InternalDns,
            (Self::ZoneRoot, None) => ApiKind::ZoneRoot,
            (Self::Zone, Some(name)) => ApiKind::Zone { name },
            (Self::Debug, None) => ApiKind::Debug,
            (Self::Zone, None) => {
                return Err(Error::internal_error("Zone kind needs name"))
            }
            (_, Some(_)) => {
                return Err(Error::internal_error("Only zone kind needs name"))
            }
        };

        Ok(k)
    }
}

impl From<&internal::shared::DatasetKind> for DatasetKind {
    fn from(k: &internal::shared::DatasetKind) -> Self {
        match k {
            internal::shared::DatasetKind::Crucible => DatasetKind::Crucible,
            internal::shared::DatasetKind::Cockroach => DatasetKind::Cockroach,
            internal::shared::DatasetKind::Clickhouse => {
                DatasetKind::Clickhouse
            }
            internal::shared::DatasetKind::ClickhouseKeeper => {
                DatasetKind::ClickhouseKeeper
            }
            internal::shared::DatasetKind::ExternalDns => {
                DatasetKind::ExternalDns
            }
            internal::shared::DatasetKind::InternalDns => {
                DatasetKind::InternalDns
            }
            internal::shared::DatasetKind::ZoneRoot => DatasetKind::ZoneRoot,
            // Enums in the database do not have associated data, so this drops
            // the "name" of the zone and only considers the type.
            //
            // The zone name, if it exists, is stored in a separate column.
            internal::shared::DatasetKind::Zone { .. } => DatasetKind::Zone,
            internal::shared::DatasetKind::Debug => DatasetKind::Debug,
        }
    }
}
