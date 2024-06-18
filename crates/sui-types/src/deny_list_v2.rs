// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::base_types::{EpochId, SuiAddress};
use crate::deny_list_v1::{
    get_deny_list_root_object, input_object_coin_types_for_denylist_check,
    DENY_LIST_COIN_TYPE_INDEX, DENY_LIST_MODULE,
};
use crate::dynamic_field::{get_dynamic_field_from_store, DOFWrapper};
use crate::error::{UserInputError, UserInputResult};
use crate::id::UID;
use crate::storage::ObjectStore;
use crate::transaction::{CheckedInputObjects, ReceivingObjects};
use crate::{MoveTypeTagTrait, SUI_FRAMEWORK_PACKAGE_ID};
use move_core_types::ident_str;
use move_core_types::language_storage::{StructTag, TypeTag};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Rust representation of the Move type 0x2::config::Config.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    id: UID,
}

/// Rust representation of the Move type 0x2::config::Setting.
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Setting<V: Clone + fmt::Debug> {
    data: Option<SettingData<V>>,
}

/// Rust representation of the Move type 0x2::config::SettingData.
#[derive(Debug, Serialize, Deserialize, Clone)]
struct SettingData<V: Clone + fmt::Debug> {
    newer_value_epoch: u64,
    newer_value: V,
    older_value_opt: Option<V>,
}

/// Rust representation of the Move type 0x2::coin::DenyCapV2.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DenyCapV2 {
    pub id: UID,
    pub allow_global_pause: bool,
}

/// Rust representation of the Move type 0x2::deny_list::ConfigKey.
#[derive(Debug, Serialize, Deserialize, Clone)]
struct ConfigKey {
    per_type_index: u64,
    per_type_key: Vec<u8>,
}

impl MoveTypeTagTrait for ConfigKey {
    fn get_type_tag() -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: SUI_FRAMEWORK_PACKAGE_ID.into(),
            module: DENY_LIST_MODULE.to_owned(),
            name: ident_str!("ConfigKey").to_owned(),
            type_params: vec![],
        }))
    }
}

/// Rust representation of the Move type 0x2::deny_list::AddressKey.
#[derive(Debug, Serialize, Deserialize, Clone)]
struct AddressKey(SuiAddress);

impl MoveTypeTagTrait for AddressKey {
    fn get_type_tag() -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: SUI_FRAMEWORK_PACKAGE_ID.into(),
            module: DENY_LIST_MODULE.to_owned(),
            name: ident_str!("AddressKey").to_owned(),
            type_params: vec![],
        }))
    }
}

pub fn check_coin_deny_list_v2_during_signing(
    address: SuiAddress,
    input_objects: &CheckedInputObjects,
    receiving_objects: &ReceivingObjects,
    object_store: &dyn ObjectStore,
) -> UserInputResult {
    let coin_types = input_object_coin_types_for_denylist_check(input_objects, receiving_objects);
    for coin_type in coin_types {
        // TODO: Check global pause flag.
        let Some(deny_list) = get_per_type_coin_deny_list_v2(&coin_type, object_store) else {
            return Ok(());
        };
        if check_address_denied_by_coin(&deny_list, address, object_store, None) {
            return Err(UserInputError::AddressDeniedForCoin { address, coin_type });
        }
    }
    Ok(())
}

pub fn get_per_type_coin_deny_list_v2(
    coin_type: &String,
    object_store: &dyn ObjectStore,
) -> Option<Config> {
    let deny_list_root =
        get_deny_list_root_object(object_store).expect("Deny list root object not found");
    let config_key = DOFWrapper {
        name: ConfigKey {
            per_type_index: DENY_LIST_COIN_TYPE_INDEX,
            per_type_key: coin_type.as_bytes().to_vec(),
        },
    };
    let config: Config =
        get_dynamic_field_from_store(object_store, deny_list_root.id(), &config_key).ok()?;
    Some(config)
}

pub fn check_address_denied_by_coin(
    coin_deny_config: &Config,
    address: SuiAddress,
    object_store: &dyn ObjectStore,
    cur_epoch: Option<EpochId>,
) -> bool {
    let address_key = AddressKey(address);
    read_config_setting(object_store, coin_deny_config, address_key, cur_epoch).unwrap_or(false)
}

/// Read the setting value from the config object in the object store for the given setting name.
/// If the setting is not found, return None.
/// If the setting is found, and if cur_epoch is not specified, it means we always want the latest value,
/// so return the newer value.
/// If the setting is found, and if cur_epoch is specified, it means we want the value prior to cur_epoch,
/// so return the older value if the newer_value_epoch is equal to or newer than cur_epoch,
/// otherwise return the newer value.
fn read_config_setting<K, V>(
    object_store: &dyn ObjectStore,
    config: &Config,
    setting_name: K,
    cur_epoch: Option<EpochId>,
) -> Option<V>
where
    K: MoveTypeTagTrait + Serialize + DeserializeOwned + fmt::Debug,
    V: Clone + fmt::Debug + Serialize + DeserializeOwned,
{
    let setting: Setting<V> = {
        match get_dynamic_field_from_store(object_store, *config.id.object_id(), &setting_name) {
            Ok(setting) => setting,
            Err(_) => return None,
        }
    };
    let Some(setting_data) = setting.data else {
        return None;
    };
    if let Some(cur_epoch) = cur_epoch {
        if setting_data.newer_value_epoch < cur_epoch {
            Some(setting_data.newer_value)
        } else {
            setting_data.older_value_opt
        }
    } else {
        Some(setting_data.newer_value)
    }
}
