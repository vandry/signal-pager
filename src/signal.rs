use comprehensive::ResourceDependencies;
use comprehensive::v1::{AssemblyRuntime, Resource, resource};
use pin_project_lite::pin_project;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::task::{JoinError, JoinHandle};

const INITIAL_RECEIVE_DELAY: Duration = Duration::new(3600, 0);
const RECEIVE_INTERVAL: Duration = Duration::new(86400, 0);

#[derive(Debug, thiserror::Error)]
pub enum SignalRunnerError {
    #[error("No state loaded")]
    NoStateAvailable,
    #[error("Error running Signal: {0}")]
    IOError(#[from] std::io::Error),
    #[error("Error joining Signal: {0}")]
    JoinError(#[from] JoinError),
    #[error("Signal exited with code {0:?}")]
    SignalFailed(Option<i32>),
}

impl From<SignalRunnerError> for (http::StatusCode, String) {
    fn from(e: SignalRunnerError) -> (http::StatusCode, String) {
        (http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

#[derive(ResourceDependencies)]
pub struct SignalRunnerDependencies(Arc<crate::state::SignalState>);

#[derive(clap::Args)]
pub struct SignalRunnerArgs {
    #[arg(long)]
    signal_phone_number: String,
    #[arg(long)]
    signal_group_id: String,
    #[arg(long)]
    signal_bin: PathBuf,
}

pub struct SignalRunner(SignalRunnerDependencies, SignalRunnerArgs);

#[resource]
impl Resource for SignalRunner {
    fn new(
        d: SignalRunnerDependencies,
        a: SignalRunnerArgs,
        api: &mut AssemblyRuntime<'_>,
    ) -> Result<Arc<Self>, std::convert::Infallible> {
        let shared = Arc::new(Self(d, a));
        let shared_for_receive = Arc::clone(&shared);
        api.set_task(async move {
            tokio::time::sleep(INITIAL_RECEIVE_DELAY).await;
            loop {
                log::info!("Invoking Signal receive");
                if let Err(e) = shared_for_receive.receive().await {
                    log::error!("Signal receive: {e}");
                }
                tokio::time::sleep(RECEIVE_INTERVAL).await;
            }
        });
        Ok(shared)
    }
}

pin_project! {
    struct ChildDriver {
        #[pin] writer: Option<JoinHandle<Result<(), std::io::Error>>>,
        #[pin] waiter: JoinHandle<Result<ExitStatus, std::io::Error>>,
    }
}

impl ChildDriver {
    fn new<M: AsRef<[u8]> + Send + 'static>(mut child: Child, msg: M) -> Self {
        let stdin = child.stdin.take();
        Self {
            writer: stdin
                .map(|mut f| tokio::task::spawn_blocking(move || f.write_all(msg.as_ref()))),
            waiter: tokio::task::spawn_blocking(move || child.wait()),
        }
    }
}

impl Future for ChildDriver {
    type Output = Result<Result<ExitStatus, std::io::Error>, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        if let Some(writer) = this.writer.as_mut().as_pin_mut() {
            if writer.poll(cx).is_ready() {
                this.writer.set(None);
            }
        }
        this.waiter.poll(cx)
    }
}

impl SignalRunner {
    pub async fn send<M: AsRef<[u8]> + Send + 'static>(
        &self,
        msg: M,
    ) -> Result<(), SignalRunnerError> {
        match self.0.0.get().await.path() {
            None => Err(SignalRunnerError::NoStateAvailable),
            Some(path) => {
                let status = ChildDriver::new(
                    Command::new(&self.1.signal_bin)
                        .arg("--config")
                        .arg(path)
                        .arg("--username")
                        .arg(&self.1.signal_phone_number)
                        .arg("send")
                        .arg("--group")
                        .arg(&self.1.signal_group_id)
                        .arg("--message-from-stdin")
                        .stdin(Stdio::piped())
                        .spawn()?,
                    msg,
                )
                .await??;
                if status.success() {
                    Ok(())
                } else {
                    Err(SignalRunnerError::SignalFailed(status.code()))
                }
            }
        }
    }

    pub async fn receive(&self) -> Result<(), SignalRunnerError> {
        match self.0.0.get().await.path() {
            None => Err(SignalRunnerError::NoStateAvailable),
            Some(path) => {
                let mut child = Command::new(&self.1.signal_bin)
                    .arg("--config")
                    .arg(path)
                    .arg("--username")
                    .arg(&self.1.signal_phone_number)
                    .arg("receive")
                    .spawn()?;
                let status = tokio::task::spawn_blocking(move || child.wait()).await??;
                if status.success() {
                    Ok(())
                } else {
                    Err(SignalRunnerError::SignalFailed(status.code()))
                }
            }
        }
    }
}
