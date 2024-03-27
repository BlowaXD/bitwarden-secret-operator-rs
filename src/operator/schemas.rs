use chrono::{DateTime, Utc};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(CustomResource, Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[kube(
    group = "bitwarden-secret-operator.io",
    version = "v1beta1",
    kind = "BitwardenSecret",
    namespaced
)]
#[kube(status = "BitwardenSecretStatus")]
#[serde(rename_all = "camelCase")]
pub struct BitwardenSecretSpec {
    #[serde(rename = "name")]
    pub name: Option<String>,

    #[serde(rename = "namespace")]
    pub namespace: Option<String>,

    #[serde(rename = "type")]
    pub secret_type: Option<String>,

    #[serde(rename = "bitwardenId")]
    pub bitwarden_id: Option<String>,

    #[serde(rename = "labels")]
    pub labels: Option<HashMap<String, String>>,

    #[serde(rename = "content")]
    pub content: Vec<ContentEntry>,

    #[serde(rename = "stringData")]
    pub string_data: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct BitwardenSecretStatus {
    pub checksum: String,
    pub last_updated: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContentEntry {
    #[serde(rename = "bitwardenId")]
    pub bitwarden_id: Option<String>,
    #[serde(rename = "bitwardenSecretField")]
    pub bitwarden_secret_field: Option<String>,
    #[serde(rename = "bitwardenUseNote")]
    pub bitwarden_use_note: Option<bool>,
    #[serde(rename = "kubernetesSecretKey")]
    pub kubernetes_secret_key: String,
    #[serde(rename = "kubernetesSecretValue")]
    pub kubernetes_secret_value: Option<String>,
}

#[derive(Error, Debug)]
pub enum BitwardenSecretError {
    #[error("The given Kubernetes secret key seems misconfigured {0}")]
    MissingBitwardenId(String),

    #[error("Bitwarden Item: {0} not found")]
    BitwardenItemNotFound(String),

    #[error("Bitwarden Item: {0}, error on field: {1}")]
    WrongValues(String, String),
}

pub(crate) const OPERATOR_HASH_LABEL: &str = "bitwarden-secret-operator-rs.io/hash";
pub(crate) const OPERATOR_LAST_UPDATE_LABEL: &str = "bitwarden-secret-operator-rs.io/last-update";
