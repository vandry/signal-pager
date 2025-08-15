use axum::extract::State;
use axum::{Json, Router};
use comprehensive::ResourceDependencies;
use comprehensive::v1::{AssemblyRuntime, Resource, resource};
use comprehensive_http::server::HttpServingInstance;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct AlertInput {
    status: String,
    labels: HashMap<String, String>,
    annotations: HashMap<String, String>,
}

impl std::fmt::Display for AlertInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        writeln!(f, "{}", self.status.to_uppercase())?;
        for (k, v) in &self.labels {
            writeln!(f, "{k}: {v}")?;
        }
        if let Some(v) = self.annotations.get("summary") {
            write!(f, "\n{v}\n")?;
        }
        if let Some(v) = self.annotations.get("description") {
            write!(f, "\n{v}\n")?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct AlertsInput {
    alerts: Vec<AlertInput>,
}

async fn alert(
    State(runner): State<Arc<crate::signal::SignalRunner>>,
    Json(payload): Json<AlertsInput>,
) -> Result<(), (http::StatusCode, String)> {
    for alert in payload.alerts {
        runner.send(format!("{alert}")).await?;
    }
    Ok(())
}

#[derive(HttpServingInstance)]
#[flag_prefix = "receiver-"]
pub struct HttpApi(#[router] Router);

#[derive(ResourceDependencies)]
pub struct HttpApiDependencies {
    signal: Arc<crate::signal::SignalRunner>,
}

#[resource]
impl Resource for HttpApi {
    fn new(
        d: HttpApiDependencies,
        _: comprehensive::NoArgs,
        _: &mut AssemblyRuntime<'_>,
    ) -> Result<Arc<Self>, std::convert::Infallible> {
        let app = Router::new()
            .route("/alert", axum::routing::post(alert))
            .with_state(d.signal);
        Ok(Arc::new(Self(app)))
    }
}
