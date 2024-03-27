use axum::routing::get;
use axum::Router;
use kube::Client;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::env;
use std::future::ready;
use std::sync::Arc;
use tokio::join;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{filter, Layer};

use crate::bitwarden_cli::BitwardenCliClient;
use crate::operator::controller::BitwardenOperator;

pub mod bitwarden_cli;
pub mod monitoring;
pub mod operator;

fn setup_metrics_recorder() -> PrometheusHandle {
    PrometheusBuilder::new().install_recorder().unwrap()
}

async fn health() -> &'static str {
    "Hello, World!"
}

async fn start_metrics_server() {
    let recorder_handle = setup_metrics_recorder();
    let app = Router::new()
        .route("/metrics", get(move || ready(recorder_handle.render())))
        .route("/health", get(health));

    let metrics_endpoint =
        env::var("METRICS_ENDPOINT").unwrap_or_else(|_| "127.0.0.1:3001".to_string());
    let listener = tokio::net::TcpListener::bind(metrics_endpoint)
        .await
        .unwrap();
    info!(
        "HTTP /metrics server listening on: {}",
        listener.local_addr().unwrap()
    );
    axum::serve(listener, app).await.unwrap();
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let stdout_log = tracing_subscriber::fmt::layer();

    let stderr_log = tracing_subscriber::fmt::layer();
    let tracer = monitoring::init_tracer().await;

    let registry = tracing_subscriber::Registry::default()
        .with(stdout_log.with_filter(filter::LevelFilter::INFO))
        .with(stderr_log.with_filter(filter::LevelFilter::ERROR));

    if let Some(tracer) = tracer {
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
        registry.with(telemetry).init();
    } else {
        registry.init();
    }

    let cli = Arc::new(BitwardenCliClient::from_env()?);
    cli.login().await?;
    cli.unlock().await?;
    cli.sync().await?;

    let client = Client::try_default().await?;

    let bitwarden_operator = BitwardenOperator::new(cli, client);
    let (_operator, _metrics_server) = join!(bitwarden_operator.start(), start_metrics_server());
    Ok(())
}
