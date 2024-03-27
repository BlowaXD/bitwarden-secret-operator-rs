use tracing::info;

pub(crate) async fn init_tracer() -> Option<opentelemetry_sdk::trace::Tracer> {
    let Ok(otlp_endpoint) = std::env::var("OPENTELEMETRY_ENDPOINT_URL") else {
        return None;
    };

    info!("Initializing OpenTelemetry Traces client");
    let channel = tonic::transport::Channel::from_shared(otlp_endpoint)
        .unwrap()
        .connect()
        .await
        .unwrap();

    Some(
        opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_channel(channel),
            )
            .with_trace_config(opentelemetry_sdk::trace::config().with_resource(
                opentelemetry_sdk::Resource::new(vec![opentelemetry::KeyValue::new(
                    "service.name",
                    "bitwarden-secret-operator-rs",
                )]),
            ))
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .unwrap(),
    )
}
