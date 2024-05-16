// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::ActionRegistry;
use super::NexusActionContext;
use super::NexusSaga;
use crate::app::sagas::declare_saga_actions;
use crate::app::sagas::ActionError;
use db::lookup::LookupPath;
use nexus_db_model::Generation;
use nexus_db_model::InstanceRuntimeState;
use nexus_db_queries::db::identity::Resource;
use nexus_db_queries::{authn, authz, db};
use omicron_common::api::external;
use omicron_common::api::external::Error;
use omicron_common::api::external::ResourceType;
use serde::{Deserialize, Serialize};
use slog::info;

/// Parameters to the instance update VMM destroyed sub-saga.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Params {
    /// Authentication context to use to fetch the instance's current state from
    /// the database.
    pub serialized_authn: authn::saga::Serialized,

    pub instance: db::model::Instance,

    pub vmm: db::model::Vmm,
}

// instance update VMM destroyed subsaga: actions

declare_saga_actions! {
    instance_update_destroyed;

    DELETE_SLED_RESOURCE -> "no_result1" {
        + siud_delete_sled_resource
    }

    DELETE_VIRTUAL_PROVISIONING -> "no_result2" {
        + siud_delete_virtual_provisioning
    }

    DELETE_V2P_MAPPINGS -> "no_result3" {
        + siud_delete_v2p_mappings
    }

    DELETE_NAT_ENTRIES -> "no_result4" {
        + siud_delete_nat_entries
    }

    UPDATE_INSTANCE -> "no_result5" {
        + siud_update_instance
    }

    MARK_VMM_DELETED -> "no_result6" {
        + siud_mark_vmm_deleted
    }
}

#[derive(Debug)]
pub(crate) struct SagaVmmDestroyed;
impl NexusSaga for SagaVmmDestroyed {
    const NAME: &'static str = "instance-update-vmm-destroyed";
    type Params = Params;

    fn register_actions(registry: &mut ActionRegistry) {
        instance_update_destroyed_register_actions(registry);
    }

    fn make_saga_dag(
        _params: &Self::Params,
        mut builder: steno::DagBuilder,
    ) -> Result<steno::Dag, super::SagaInitError> {
        builder.append(delete_sled_resource_action());
        builder.append(delete_virtual_provisioning_action());
        builder.append(delete_v2p_mappings_action());
        builder.append(delete_nat_entries_action());
        builder.append(update_instance_action());
        builder.append(mark_vmm_deleted_action());

        Ok(builder.build()?)
    }
}

async fn siud_delete_sled_resource(
    sagactx: NexusActionContext,
) -> Result<(), ActionError> {
    let osagactx = sagactx.user_data();
    let Params { ref serialized_authn, ref vmm, ref instance, .. } =
        sagactx.saga_params::<Params>()?;

    let opctx =
        crate::context::op_context_for_saga_action(&sagactx, serialized_authn);

    info!(
        osagactx.log(),
        "instance update (VMM destroyed): deleting sled reservation";
        "instance_id" => %instance.id(),
        "propolis_id" => %vmm.id,
        "instance_update" => %"VMM destroyed",
    );

    osagactx
        .datastore()
        .sled_reservation_delete(&opctx, vmm.id)
        .await
        .or_else(|err| {
            // Necessary for idempotency
            match err {
                Error::ObjectNotFound { .. } => Ok(()),
                _ => Err(err),
            }
        })
        .map_err(ActionError::action_failed)
}

async fn siud_delete_virtual_provisioning(
    sagactx: NexusActionContext,
) -> Result<(), ActionError> {
    let osagactx = sagactx.user_data();
    let Params { ref serialized_authn, ref instance, ref vmm, .. } =
        sagactx.saga_params::<Params>()?;

    let opctx =
        crate::context::op_context_for_saga_action(&sagactx, serialized_authn);

    info!(
        osagactx.log(),
        "instance update (VMM destroyed): deleting virtual provisioning";
        "instance_id" => %instance.id(),
        "propolis_id" => %vmm.id,
        "instance_update" => %"VMM destroyed",
    );

    osagactx
        .datastore()
        .virtual_provisioning_collection_delete_instance(
            &opctx,
            instance.id(),
            instance.project_id,
            i64::from(instance.ncpus.0 .0),
            instance.memory,
            i64::try_from(&instance.runtime_state.gen.0).unwrap(),
        )
        .await
        .map(|_| ())
        .or_else(|err| {
            // Necessary for idempotency
            match err {
                Error::ObjectNotFound { .. } => Ok(()),
                _ => Err(ActionError::action_failed(err)),
            }
        })
}

async fn siud_delete_v2p_mappings(
    sagactx: NexusActionContext,
) -> Result<(), ActionError> {
    let osagactx = sagactx.user_data();
    let Params { ref serialized_authn, ref instance, ref vmm, .. } =
        sagactx.saga_params::<Params>()?;

    let opctx =
        crate::context::op_context_for_saga_action(&sagactx, serialized_authn);

    info!(
        osagactx.log(),
        "instance update (VMM destroyed): deleting V2P mappings";
        "instance_id" => %instance.id(),
        "propolis_id" => %vmm.id,
        "instance_update" => %"VMM destroyed",
    );

    // Per the commentary in instance_network::delete_instance_v2p_mappings`,
    // this should be idempotent.
    osagactx
        .nexus()
        .delete_instance_v2p_mappings(&opctx, instance.id())
        .await
        .or_else(|err| {
            // Necessary for idempotency
            match err {
                Error::ObjectNotFound {
                    type_name: ResourceType::Instance,
                    lookup_type: _,
                } => Ok(()),
                _ => Err(ActionError::action_failed(err)),
            }
        })
}

async fn siud_delete_nat_entries(
    sagactx: NexusActionContext,
) -> Result<(), ActionError> {
    let osagactx = sagactx.user_data();
    let Params { ref serialized_authn, ref vmm, ref instance, .. } =
        sagactx.saga_params::<Params>()?;

    let opctx =
        crate::context::op_context_for_saga_action(&sagactx, serialized_authn);

    info!(
        osagactx.log(),
        "instance update (VMM destroyed): deleting NAT entries";
        "instance_id" => %instance.id(),
        "propolis_id" => %vmm.id,
        "instance_update" => %"VMM destroyed",
    );

    let (.., authz_instance) = LookupPath::new(&opctx, &osagactx.datastore())
        .instance_id(instance.id())
        .lookup_for(authz::Action::Modify)
        .await
        .map_err(ActionError::action_failed)?;
    osagactx
        .nexus()
        .instance_delete_dpd_config(&opctx, &authz_instance)
        .await
        .map_err(ActionError::action_failed)?;
    Ok(())
}

async fn siud_update_instance(
    sagactx: NexusActionContext,
) -> Result<(), ActionError> {
    let osagactx = sagactx.user_data();
    let Params { instance, vmm, .. } = sagactx.saga_params::<Params>()?;
    let new_runtime = InstanceRuntimeState {
        propolis_id: None,
        nexus_state: external::InstanceState::Stopped.into(),
        gen: Generation(instance.runtime_state.gen.0.next()),
        ..instance.runtime_state
    };

    info!(
        osagactx.log(),
        "instance update (VMM destroyed): updating runtime state";
        "instance_id" => %instance.id(),
        "propolis_id" => %vmm.id,
        "new_runtime_state" => ?new_runtime,
        "instance_update" => %"VMM destroyed",
    );

    // It's okay for this to fail, it just means that the active VMM ID has changed.
    let _ = osagactx
        .datastore()
        .instance_update_runtime(&instance.id(), &new_runtime)
        .await;
    Ok(())
}

async fn siud_mark_vmm_deleted(
    sagactx: NexusActionContext,
) -> Result<(), ActionError> {
    let osagactx = sagactx.user_data();
    let Params { ref serialized_authn, ref vmm, ref instance, .. } =
        sagactx.saga_params::<Params>()?;

    let opctx =
        crate::context::op_context_for_saga_action(&sagactx, serialized_authn);

    info!(
        osagactx.log(),
        "instance update (VMM destroyed): marking VMM record deleted";
        "instance_id" => %instance.id(),
        "propolis_id" => %vmm.id,
        "instance_update" => %"VMM destroyed",
    );

    osagactx
        .datastore()
        .vmm_mark_deleted(&opctx, &vmm.id)
        .await
        .map(|_| ())
        .map_err(ActionError::action_failed)
}
