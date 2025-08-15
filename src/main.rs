use comprehensive::ResourceDependencies;
use comprehensive_http::HttpServer;
use std::sync::Arc;

mod http;
mod signal;
mod state;

#[derive(ResourceDependencies)]
struct TopDependencies {
    _http: Arc<HttpServer<http::HttpApi>>,
    _diag: Arc<comprehensive_http::diag::HttpServer>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    comprehensive::Assembly::<TopDependencies>::new()?
        .run()
        .await?;
    Ok(())
}
