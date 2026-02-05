use comprehensive_http::HttpServer;
use std::marker::PhantomData;
use std::sync::Arc;

mod grpc;
mod http;
mod signal;
mod state;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    comprehensive::Assembly::<(
        Arc<HttpServer<http::HttpApi>>,
        Arc<comprehensive_http::diag::HttpServer>,
        Arc<comprehensive_grpc::server::GrpcServer>,
        PhantomData<grpc::PagerService>,
        PhantomData<comprehensive_spiffe::SpiffeTlsProvider>,
    )>::new()?
    .run()
    .await?;
    Ok(())
}
