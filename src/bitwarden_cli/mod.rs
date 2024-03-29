use crate::bitwarden_cli::BitwardenError::MissingEnvVariable;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::RwLock;
use tonic::async_trait;
use tracing::{error, info};

#[derive(Debug, Clone, Default)]
pub struct BitwardenCliWrapperStorage {
    session_token: Option<String>,

    last_unlock: Option<DateTime<Utc>>,
    last_sync: Option<DateTime<Utc>>,

    needs_relog: bool,
}

#[derive(Debug, Clone)]
pub struct BitwardenCliClient {
    client_id: String,
    client_secret: String,
    client_password: String,

    storage: Arc<RwLock<BitwardenCliWrapperStorage>>,
}

#[derive(Error, Debug)]
pub enum BitwardenError {
    #[error("missing env variable {0}")]
    MissingEnvVariable(String),
    #[error("bw login failed")]
    LoginFailed(String),
    #[error("`bw sync` failed")]
    SyncFailed,
    #[error("`bw sync` failed because session token was not initialized")]
    SyncFailedTokenMissing,
    #[error("`bw unlock` failed")]
    UnlockFailed,
    #[error("bw get item failed: {0}, not found")]
    ItemNotFound(String),
    #[error("bw get item failed: {0}, error: {1}")]
    GetItemGenericFail(String, String),
    #[error("bitwarden command: {0} failed")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BitwardenItem {
    pub id: String,
    #[serde(rename = "notes")]
    pub note: Option<String>,
    pub fields: Option<Vec<BitwardenItemField>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BitwardenItemField {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BitwardenGetItemResponse {
    pub data: Option<BitwardenItem>,
    pub success: bool,
}

const BW_CLIENTID: &str = "BW_CLIENTID";
const BW_CLIENTSECRET: &str = "BW_CLIENTSECRET";
const BW_PASSWORD: &str = "BW_PASSWORD";

#[async_trait]
pub trait SecretStoreGetItem {
    type Error;
    async fn get_item(&mut self, item_id: String) -> Result<(), Self::Error>;
}

#[async_trait]
pub trait SecretStoreSynchronize {
    type Error;
    async fn sync(&mut self) -> Result<(), Self::Error>;
}

impl BitwardenCliClient {
    pub fn from_env() -> eyre::Result<Self> {
        Ok(BitwardenCliClient {
            client_id: env::var(BW_CLIENTID)
                .map_err(|_| MissingEnvVariable(BW_CLIENTID.to_string()))?
                .to_string(),
            client_secret: env::var(BW_CLIENTSECRET)
                .map_err(|_| MissingEnvVariable(BW_CLIENTSECRET.to_string()))?
                .to_string(),
            client_password: env::var(BW_PASSWORD)
                .map_err(|_| MissingEnvVariable(BW_PASSWORD.to_string()))?
                .to_string(),
            storage: Arc::new(RwLock::new(BitwardenCliWrapperStorage::default())),
        })
    }

    pub async fn login(&self) -> eyre::Result<(), BitwardenError> {
        let client_id = self.client_id.clone();
        let client_secret = self.client_secret.clone();

        info!("`bw login`");
        let output = tokio::process::Command::new("bw")
            .args(["login", "--apikey", "--nointeraction"])
            .env(BW_CLIENTID, client_id)
            .env(BW_CLIENTSECRET, client_secret)
            .output()
            .await?;

        let exit_status = output.status.code().unwrap_or_default();
        match exit_status {
            0 => {
                // success
                info!("Successfully logged in");
                Ok(())
            }
            1 => {
                // error code 1, handling "Already logged in" scenario
                let stderr = String::from_utf8(output.stderr)
                    .map_err(|_| BitwardenError::LoginFailed("Couldn't get stderr".to_string()))?;

                if !stderr.starts_with("You are already logged in as") {
                    return Err(BitwardenError::LoginFailed(
                        "Login Error: CLI returned exitCode 1 but not 'already logged in'"
                            .to_string(),
                    ));
                }

                Ok(())
            }
            x => {
                error!("Login Error: CLI returned unhandled exitCode: {}", x);
                Err(BitwardenError::LoginFailed(format!(
                    "Login Error: CLI returned unhandled exitCode: {}",
                    x
                )))
            }
        }
    }

    pub async fn unlock(&self) -> Result<(), BitwardenError> {
        let client_id = self.client_id.clone();
        let client_secret = self.client_secret.clone();
        let client_password = self.client_password.clone();

        info!("`bw unlock`");
        let cmd = tokio::process::Command::new("bw")
            .args(["unlock", "--passwordenv", "BW_PASSWORD", "--nointeraction"])
            .env("BW_CLIENTID", client_id)
            .env("BW_CLIENTSECRET", client_secret)
            .env("BW_PASSWORD", client_password)
            .output()
            .await;

        match cmd {
            Ok(output) => {
                let output_str = String::from_utf8(output.stdout).unwrap();
                let session_text = "BW_SESSION=\"";
                let begin = output_str.find(session_text).unwrap() + session_text.len();
                let end = output_str[begin..].find('"').unwrap();
                let session_token = output_str[begin..(begin + end)].to_string();

                let mut storage = self.storage.write().await;
                storage.session_token = Some(session_token);
                storage.last_unlock = Some(chrono::offset::Utc::now());

                info!("`bw unlock` succeed");
                Ok(())
            }
            Err(err) => {
                error!("`bw unlock` failed, {}", err.to_string());
                Err(BitwardenError::UnlockFailed)
            }
        }
    }

    pub async fn sync(&self) -> Result<(), BitwardenError> {
        let mut storage = self.storage.write().await;
        let Some(session_token) = &storage.session_token else {
            return Err(BitwardenError::SyncFailedTokenMissing);
        };

        info!("`bw sync`");
        let cmd = tokio::process::Command::new("bw")
            .args(["sync"])
            .env("BW_SESSION", session_token.clone())
            .output()
            .await;

        match cmd {
            Ok(_) => {
                info!("`bw sync` succeed");

                storage.last_sync = Some(chrono::offset::Utc::now());
                Ok(())
            }
            Err(err) => {
                error!("`bw sync` failed, {}", err.to_string());
                storage.needs_relog = true;
                Err(BitwardenError::SyncFailed)
            }
        }
    }

    pub async fn get_item(&self, item_id: String) -> Result<BitwardenItem, BitwardenError> {
        let mut storage = self.storage.write().await;
        let Some(session_token) = &storage.session_token else {
            return Err(BitwardenError::SyncFailedTokenMissing);
        };

        let cmd = tokio::process::Command::new("bw")
            .args(["--response", "get", "item", &item_id, "--nointeraction"])
            .env("BW_SESSION", session_token)
            .output()
            .await;

        match cmd {
            Ok(output) => {
                let response =
                    serde_json::from_slice::<BitwardenGetItemResponse>(output.stdout.as_slice());

                if response.is_err() {
                    let error_msg = String::from_utf8(output.stdout).unwrap();
                    error!(
                        "`bw get item {}` failed: {}, body: {}",
                        item_id,
                        response.unwrap_err(),
                        error_msg
                    );
                    return Err(BitwardenError::ItemNotFound(item_id));
                }

                let data = response.unwrap();
                if !data.success {
                    error!("`bw get item {}` failed, couldn't find item", item_id);
                    return Err(BitwardenError::ItemNotFound(item_id));
                }

                if data.data.is_none() {
                    error!("`bw get item {}` failed, couldn't find item", item_id);
                    return Err(BitwardenError::ItemNotFound(item_id));
                }

                info!("`bw get item {item_id}` succeed");

                Ok(data.data.unwrap())
            }
            Err(err) => {
                error!("`bw get item {}` failed, {}", item_id, err.to_string());
                storage.needs_relog = true;
                Err(BitwardenError::GetItemGenericFail(item_id, err.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bitwarden_cli::BitwardenItem;
    use std::fs;

    const BITWARDEN_FIELDS: &str = "tests/bitwarden-fields.json";
    const BITWARDEN_NOTES: &str = "tests/bitwarden-note.json";

    #[test]
    fn deserialize_bitwarden_fields() -> eyre::Result<()> {
        let bitwarden_item = fs::read_to_string(BITWARDEN_FIELDS)
            .unwrap_or_else(|_| panic!("Couldn't deserialize {BITWARDEN_FIELDS}"));

        let bitwarden_item: BitwardenItem =
            serde_json::from_str(&bitwarden_item).expect("Couldn't deserialize to BitwardenItem");

        let fields = bitwarden_item.fields.expect("Couldn't deserialize fields");
        assert_eq!(bitwarden_item.id, "00000000-0000-0000-0000-000000000000");
        assert_eq!(fields[0].name, "super-secret-field");
        assert_eq!(fields[0].value, "super-secret");
        assert_eq!(bitwarden_item.note, None);
        Ok(())
    }

    #[test]
    fn deserialize_bitwarden_notes() -> eyre::Result<()> {
        let bitwarden_item = fs::read_to_string(BITWARDEN_NOTES)
            .unwrap_or_else(|_| panic!("Couldn't deserialize {BITWARDEN_NOTES}"));

        let bitwarden_item: BitwardenItem =
            serde_json::from_str(&bitwarden_item).expect("Couldn't deserialize to BitwardenItem");

        assert_eq!(bitwarden_item.id, "00000000-0000-0000-0000-000000000000");
        assert!(bitwarden_item.fields.is_none());
        assert_eq!(bitwarden_item.note.unwrap(), "hello-world");
        Ok(())
    }
}
