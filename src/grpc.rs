use comprehensive::v1::{AssemblyRuntime, Resource, resource};
use itertools::Itertools;
use std::collections::HashSet;
use std::sync::Arc;
use tonic::{Code, Status};
use x509_parser::certificate::X509Certificate;
use x509_parser::prelude::FromDer;

mod pb {
    tonic::include_proto!("pager");
    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("fdset");
}

pub struct PagerService {
    signal: Arc<crate::signal::SignalRunner>,
    acl: HashSet<String>,
}

#[derive(clap::Args)]
pub struct PagerServiceArgs {
    #[arg(long)]
    allow_spiffe: Vec<String>,
}

#[resource]
#[export_grpc(pb::pager_server::PagerServer)]
#[proto_descriptor(pb::FILE_DESCRIPTOR_SET)]
impl Resource for PagerService {
    fn new(
        d: (Arc<crate::signal::SignalRunner>,),
        args: PagerServiceArgs,
        _: &mut AssemblyRuntime<'_>,
    ) -> Result<Arc<Self>, std::convert::Infallible> {
        Ok(Arc::new(Self {
            signal: d.0,
            acl: args.allow_spiffe.into_iter().collect(),
        }))
    }
}

#[tonic::async_trait]
impl pb::pager_server::Pager for PagerService {
    async fn page(
        &self,
        req: tonic::Request<pb::PageRequest>,
    ) -> Result<tonic::Response<()>, Status> {
        let certs = req
            .peer_certs()
            .ok_or_else(|| Status::new(Code::PermissionDenied, "no client certificate"))?;
        let cert = certs
            .iter()
            .next()
            .ok_or_else(|| Status::new(Code::PermissionDenied, "no client certificate"))?;
        let x509 = X509Certificate::from_der(cert)
            .map_err(|e| {
                Status::new(
                    Code::PermissionDenied,
                    format!("error reading client certificate: {}", e),
                )
            })?
            .1;
        let cn = x509
            .subject_alternative_name()
            .ok()
            .flatten()
            .and_then(|ext| ext.value.general_names.iter().exactly_one().ok())
            .and_then(|gn| match gn {
                x509_parser::extensions::GeneralName::URI(s) => Some(*s),
                _ => None,
            })
            .ok_or_else(|| Status::new(Code::PermissionDenied, "no URI SAN in certificate"))?;
        if self.acl.get(cn).is_none() {
            return Err(Status::new(Code::PermissionDenied, "not in ACL"));
        }

        self.signal
            .send(req.into_inner().message.unwrap_or_default())
            .await?;
        Ok(tonic::Response::new(()))
    }
}
