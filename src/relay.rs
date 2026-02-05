use comprehensive_http::HttpServer;
use std::marker::PhantomData;
use std::sync::Arc;

mod http;

mod signal {
    use comprehensive::v1::{AssemblyRuntime, Resource, resource};
    use comprehensive_grpc::GrpcClient;
    use std::sync::Arc;
    use tonic::Code;

    mod pb {
        tonic::include_proto!("pager");
    }

    #[derive(GrpcClient)]
    pub struct Client(pb::pager_client::PagerClient<comprehensive_grpc::client::Channel>);

    pub struct SignalRunner(Arc<Client>);

    #[resource]
    impl Resource for SignalRunner {
        fn new(
            (client_resource,): (Arc<Client>,),
            _: comprehensive::NoArgs,
            _: &mut AssemblyRuntime<'_>,
        ) -> Result<Arc<Self>, std::convert::Infallible> {
            Ok(Arc::new(Self(client_resource)))
        }
    }

    impl SignalRunner {
        pub async fn send(&self, msg: String) -> Result<(), (http::StatusCode, String)> {
            match self
                .0
                .client()
                .page(pb::PageRequest { message: Some(msg) })
                .await
            {
                Ok(_) => Ok(()),
                Err(s) => Err((
                    match s.code() {
                        Code::NotFound => http::StatusCode::NOT_FOUND,
                        Code::PermissionDenied => http::StatusCode::FORBIDDEN,
                        _ => http::StatusCode::INTERNAL_SERVER_ERROR,
                    },
                    s.to_string(),
                )),
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    comprehensive::Assembly::<(
        Arc<HttpServer<http::HttpApi>>,
        Arc<comprehensive_http::diag::HttpServer>,
        PhantomData<comprehensive_spiffe::SpiffeTlsProvider>,
    )>::new()?
    .run()
    .await?;
    Ok(())
}
