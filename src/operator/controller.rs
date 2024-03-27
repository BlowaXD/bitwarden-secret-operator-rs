use crate::bitwarden_cli::BitwardenCliClient;
use crate::operator::generate_secret_from_bitwarden_secret;
use crate::operator::schemas::{BitwardenSecret, BitwardenSecretError, BitwardenSecretStatus};
use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::api::{Patch, PatchParams, PostParams};
use kube::runtime::controller::Action;
use kube::runtime::{watcher, Controller};
use kube::{Api, Client, ResourceExt};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::{join, task};
use tracing::{error, info, warn};

pub struct BitwardenOperator {
    cli: Arc<BitwardenCliClient>,
    client: Client,
}

#[derive(Clone)]
struct KubeContext {
    /// kubernetes client
    client: Client,
    bitwarden_cli: Arc<BitwardenCliClient>,
}

impl BitwardenOperator {
    pub fn new(cli: Arc<BitwardenCliClient>, client: Client) -> Self {
        Self { cli, client }
    }

    pub async fn start(&self) -> eyre::Result<()> {
        info!("Starting Operator...");
        let context = Arc::new(KubeContext {
            client: self.client.clone(),
            bitwarden_cli: self.cli.clone(),
        });

        let cli = self.cli.clone();

        // background task to sync the CLI secrets every X seconds
        task::spawn(async move {
            let cli = cli.clone();
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let _ = cli.sync().await;
            }
        });

        // generate secret
        let bitwarden_secrets = Api::<BitwardenSecret>::all(self.client.clone());
        let secrets = Api::<Secret>::all(self.client.clone());

        Controller::new(bitwarden_secrets.clone(), watcher::Config::default())
            .owns(secrets, watcher::Config::default())
            .run(reconcile_bitwarden_secret, error_policy, context)
            .for_each(|res| async move {
                match res {
                    Ok(o) => info!("reconciled {}:{}", o.0.namespace.unwrap(), o.0.name),
                    Err(e) => warn!("reconcile failed: {}", e),
                }
            })
            .await;

        Ok(())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum BitwardenOperatorError {
    #[error("BitwardenSecretError: {0}, ({0:?})")]
    BitwardenSecretError(#[from] BitwardenSecretError),
    #[error("KubernetesClientError: {0} ({0:?})")]
    KubernetesError(#[from] kube::error::Error),
}

pub type BitwardenOperatorResult<T, E = BitwardenOperatorError> = Result<T, E>;

async fn reconcile_bitwarden_secret(
    obj: Arc<BitwardenSecret>,
    ctx: Arc<KubeContext>,
) -> BitwardenOperatorResult<Action> {
    let manifest_name = &obj.name_any();
    info!("reconcile request: {}", manifest_name);
    metrics::counter!("reconcile_requests_total").increment(1);

    // avoid refreshing if unnecessary
    if let Some(status) = &obj.status {
        // TODO configuration later
        let now = Utc::now();
        if status
            .last_updated
            .is_some_and(|x| now < x + Duration::from_secs(3600))
        {
            // TODO configuration later
            return Ok(Action::requeue(Duration::from_secs(60)));
        }
    };

    let target_namespace = &obj
        .spec
        .namespace
        .clone()
        .unwrap_or_else(|| obj.namespace().unwrap());
    let secret_name = obj.spec.name.clone().unwrap_or_else(|| obj.name_any());

    let namespace = Api::<Secret>::namespaced(ctx.client.clone(), target_namespace);
    let (present_secret_result, expected_secret_result) = join!(
        namespace.get_opt(&secret_name),
        generate_secret_from_bitwarden_secret(ctx.bitwarden_cli.clone(), obj.clone())
    );

    let secret = match expected_secret_result {
        Ok(secret) => secret,
        Err(e) => {
            // Log the error and return early with Err
            error!(
                "Failed to reconcile BitwardenSecret: {}, {}",
                manifest_name,
                e.to_string()
            );
            return Err(BitwardenOperatorError::BitwardenSecretError(e));
        }
    };

    if present_secret_result?.is_some() {
        info!(
            "Secret: {} - {} replacing...",
            secret.name_any(),
            secret.namespace().unwrap()
        );
        namespace
            .replace(
                &secret.name_any(),
                &PostParams {
                    dry_run: false,
                    field_manager: Default::default(),
                },
                &secret,
            )
            .await?;
        info!(
            "Secret: {} - {} replaced!",
            secret.name_any(),
            secret.namespace().unwrap()
        );
    } else {
        info!(
            "Secret: {} - {} creating...",
            secret.name_any(),
            secret.namespace().unwrap()
        );
        namespace
            .create(
                &PostParams {
                    dry_run: false,
                    field_manager: Default::default(),
                },
                &secret,
            )
            .await?;
        info!(
            "Secret: {} - {} created!",
            secret.name_any(),
            secret.namespace().unwrap()
        );
    }

    let status = json!({
        "status": BitwardenSecretStatus {
            checksum: "todo".to_string(),
            last_updated: Some(Utc::now()),
        }
    });

    let namespace = &obj.namespace().unwrap();
    info!("BitwardenSecret: {} updating status...", obj.name_any());
    let api = Api::<BitwardenSecret>::namespaced(ctx.client.clone(), namespace);
    api.patch_status(
        &obj.name_any(),
        &PatchParams::default(),
        &Patch::Merge(&status),
    )
    .await?;
    info!("BitwardenSecret: {} status updated!", obj.name_any());

    metrics::counter!("reconcile_requests_success_total").increment(1);
    Ok(Action::await_change())
}

fn error_policy(
    _object: Arc<BitwardenSecret>,
    _err: &BitwardenOperatorError,
    _ctx: Arc<KubeContext>,
) -> Action {
    metrics::counter!("reconcile_errors_total").increment(1);
    Action::requeue(Duration::from_secs(5))
}
