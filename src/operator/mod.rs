pub mod controller;
pub mod schemas;

use crate::bitwarden_cli::{BitwardenCliClient, BitwardenItem};
use crate::operator::schemas::{
    BitwardenSecret, BitwardenSecretError, BitwardenSecretSpec, ContentEntry,
};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::{Resource, ResourceExt};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

fn get_bitwarden_id(
    content_entry: &ContentEntry,
    bitwarden_secret: &BitwardenSecret,
) -> Result<String, BitwardenSecretError> {
    content_entry
        .bitwarden_id
        .clone()
        .or_else(|| bitwarden_secret.spec.bitwarden_id.clone())
        .ok_or_else(|| {
            BitwardenSecretError::MissingBitwardenId(content_entry.kubernetes_secret_key.clone())
        })
}

fn get_secret_value(
    content_entry: &ContentEntry,
    bitwarden_item: &BitwardenItem,
    bitwarden_id: &str,
) -> Result<String, BitwardenSecretError> {
    if let Some(value) = &content_entry.kubernetes_secret_value {
        return Ok(value.clone());
    }

    if let Some(use_note) = content_entry.bitwarden_use_note {
        return if use_note {
            // If bitwarden_use_note is true, try to return the note.
            bitwarden_item.note.clone().ok_or_else(|| {
                BitwardenSecretError::WrongValues(
                    bitwarden_id.to_string(),
                    "bitwarden_use_note".to_string(),
                )
            })
        } else {
            // If bitwarden_use_note is false, return an error.
            Err(BitwardenSecretError::WrongValues(
                bitwarden_id.to_string(),
                "bitwarden_use_note".to_string(),
            ))
        };
    }

    if let Some(field_name) = &content_entry.bitwarden_secret_field {
        if let Some(fields) = &bitwarden_item.fields {
            let item_field = fields
                .iter()
                .find(|x| &x.name == field_name)
                .ok_or_else(|| {
                    BitwardenSecretError::BitwardenItemNotFound(field_name.to_string())
                })?;
            return Ok(item_field.value.clone());
        }
    }

    Err(BitwardenSecretError::WrongValues(
        bitwarden_id.to_string(),
        "bitwarden_use_note".to_string(),
    ))
}

pub async fn generate_secret_from_bitwarden_secret(
    cli: Arc<BitwardenCliClient>,
    bitwarden_secret: Arc<BitwardenSecret>,
) -> Result<Secret, BitwardenSecretError> {
    let mut secret = Secret::default();
    secret.metadata.name = Some(
        bitwarden_secret
            .spec
            .name
            .clone()
            .unwrap_or_else(|| bitwarden_secret.metadata.name.clone().unwrap()),
    );
    secret.metadata.namespace = Some(
        bitwarden_secret
            .spec
            .namespace
            .clone()
            .unwrap_or_else(|| bitwarden_secret.metadata.namespace.clone().unwrap()),
    );

    let oref = bitwarden_secret.controller_owner_ref(&()).unwrap();
    secret.owner_references_mut().push(oref);

    let global_bitwarden_id = bitwarden_secret.spec.bitwarden_id.clone();
    let to_fetch = try_get_to_fetch(
        &bitwarden_secret,
        &bitwarden_secret.spec,
        global_bitwarden_id,
    )?;

    // get all bitwarden needed secrets
    let mut fetched = HashMap::<String, BitwardenItem>::new();
    for element in to_fetch {
        let item = cli
            .get_item(element.clone())
            .await
            .map_err(|_e| BitwardenSecretError::BitwardenItemNotFound(element.clone()))?;
        fetched.insert(element.clone(), item);
    }

    let secret_data = generate_secret_data(&bitwarden_secret, &mut fetched)?;
    secret.data = Some(secret_data);

    let mut string_data = BTreeMap::<String, String>::new();
    if let Some(bw_string_data) = &bitwarden_secret.spec.string_data {
        for x in bw_string_data {
            string_data.insert(x.0.clone(), x.1.clone());
        }
    }
    secret.string_data = Some(string_data);

    let mut labels = match secret.metadata.labels {
        Some(ref x) => x.clone(),
        None => BTreeMap::new(),
    };

    let now = chrono::offset::Utc::now();
    labels.insert(schemas::OPERATOR_HASH_LABEL.to_string(), "test".to_string());
    labels.insert(
        schemas::OPERATOR_LAST_UPDATE_LABEL.to_string(),
        now.timestamp().to_string(),
    );

    if let Some(forwarded_labels) = &bitwarden_secret.spec.labels {
        for label in forwarded_labels {
            labels.insert(label.0.clone(), label.1.clone());
        }
    }
    secret.metadata.labels = Some(labels);
    Ok(secret)
}

fn generate_secret_data(
    bitwarden_secret: &Arc<BitwardenSecret>,
    fetched: &mut HashMap<String, BitwardenItem>,
) -> Result<BTreeMap<String, ByteString>, BitwardenSecretError> {
    let mut secret_data = BTreeMap::<String, ByteString>::new();
    for entry in &bitwarden_secret.spec.content {
        let bitwarden_data = fetched
            .get(&get_bitwarden_id(entry, bitwarden_secret)?)
            .ok_or_else(|| {
                BitwardenSecretError::MissingBitwardenId(entry.kubernetes_secret_key.clone())
            })?;

        let bitwarden_id = &get_bitwarden_id(entry, bitwarden_secret)?;
        let secret_value = get_secret_value(entry, bitwarden_data, bitwarden_id)?;

        secret_data.insert(
            entry.kubernetes_secret_key.clone(),
            ByteString(secret_value.as_bytes().to_vec()),
        );
    }
    Ok(secret_data)
}

fn try_get_to_fetch(
    bitwarden_secret: &Arc<BitwardenSecret>,
    bitwarden_spec: &BitwardenSecretSpec,
    global_bitwarden_id: Option<String>,
) -> Result<HashSet<String>, BitwardenSecretError> {
    let mut to_fetch = HashSet::<String>::new();

    for content in &bitwarden_spec.content {
        if content.bitwarden_id.is_none()
            && content.kubernetes_secret_value.is_none()
            && global_bitwarden_id.is_none()
        {
            return Err(BitwardenSecretError::MissingBitwardenId(
                content.kubernetes_secret_key.clone(),
            ));
        }

        to_fetch.insert(get_bitwarden_id(content, bitwarden_secret)?);
    }
    Ok(to_fetch)
}
