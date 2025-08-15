use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, ChaCha20Poly1305, KeyInit};
use comprehensive::ResourceDependencies;
use comprehensive::v1::{AssemblyRuntime, Resource, resource};
use flate2::Compression;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use pin_project_lite::pin_project;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tempfile::TempDir;

const MAINTENANCE_INTERVAL: Duration = Duration::new(15 * 60, 0);

comprehensive_s3::bucket!(SignalStateBucket, "signal state storage", "");

#[derive(ResourceDependencies)]
pub struct SignalStateDependencies(Arc<SignalStateBucket>);

#[derive(Debug, thiserror::Error)]
pub enum SignalStateError {
    #[error("{0}")]
    S3Error(#[from] s3::error::S3Error),
    #[error("{0}")]
    IOError(#[from] std::io::Error),
    #[error("{0}")]
    CryptoError(#[from] chacha20poly1305::Error),
    #[error("No state available in S3")]
    NoStateAvailable,
    #[error("Ciphertext too short")]
    CiphertextTooShort,
    #[error("{0}")]
    InvalidKeyLength(#[from] crypto_common::InvalidLength),
}

struct Inner {
    version: u32,
    dir: TempDir,
    dirtied: AtomicBool,
}

impl Inner {
    async fn save(
        &mut self,
        cipher: &ChaCha20Poly1305,
        bucket: &s3::Bucket,
    ) -> Result<(), SignalStateError> {
        let state = pack_state(cipher, self.dir.path())?;
        self.version += 1;
        let version = self.version;
        log::info!("Persisting state as {version}");
        bucket.put_object(version.to_string(), &state).await?;
        self.dirtied.store(false, Ordering::Release);
        log::info!("Done persisting state as {version}");
        Ok(())
    }

    async fn load(
        cipher: &ChaCha20Poly1305,
        bucket: &s3::Bucket,
        version: u32,
    ) -> Result<Self, SignalStateError> {
        let ciphertext = bucket.get_object(version.to_string()).await?;
        let s = ciphertext.as_slice();
        let ns = 12; //<ChaCha20Poly1305 as AeadCore>::NonceSize;
        if s.len() <= ns {
            return Err(SignalStateError::CiphertextTooShort);
        }
        let tar_gz = cipher.decrypt((&s[0..ns]).into(), &s[ns..])?;
        let cursor = std::io::Cursor::new(&tar_gz);
        let tar = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(tar);
        let dir = tempfile::tempdir()?;
        archive.unpack(dir.path())?;
        log::info!(
            "Loaded state at version {version} into {}",
            dir.path().display()
        );
        Ok(Self {
            version,
            dir,
            dirtied: AtomicBool::new(false),
        })
    }
}

pub struct SignalState {
    inner: tokio::sync::RwLock<Option<Inner>>,
}

pub struct StateGuard<'a>(tokio::sync::RwLockReadGuard<'a, Option<Inner>>);

impl<'a> StateGuard<'a> {
    pub fn path(&'a self) -> Option<&'a Path> {
        self.0.as_ref().map(|inner| {
            inner.dirtied.store(true, Ordering::Release);
            inner.dir.path()
        })
    }
}

impl SignalState {
    pub async fn get(&self) -> StateGuard<'_> {
        StateGuard(self.inner.read().await)
    }
}

#[derive(clap::Args)]
pub struct SignalStateArgs {
    #[arg(long)]
    encryption_key: PathBuf,
    #[arg(long)]
    bootstrap: Option<PathBuf>,
}

fn pack_state<P: AsRef<Path>>(
    cipher: &ChaCha20Poly1305,
    path: P,
) -> Result<Vec<u8>, SignalStateError> {
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut tar_gz = Vec::new();
    let enc = flate2::write::GzEncoder::new(&mut tar_gz, Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all("", path)?;
    tar.finish()?;
    drop(tar);
    let ciphertext = cipher.encrypt(&nonce, &*tar_gz)?;
    Ok([nonce.as_slice(), &ciphertext]
        .into_iter()
        .flatten()
        .copied()
        .collect())
}

pin_project! {
    struct SignalStateMaintenanceRunning<S, M> {
        #[pin] stopper: S,
        #[pin] maintenance: M,
    }
}

pin_project! {
    struct SignalStateMaintenance<S, M, C> {
        #[pin] running: Option<SignalStateMaintenanceRunning<S, M>>,
        #[pin] cleanup: C,
    }
}

impl<S, M, C> SignalStateMaintenance<S, M, C> {
    fn new(stopper: S, maintenance: M, cleanup: C) -> Self {
        Self {
            running: Some(SignalStateMaintenanceRunning {
                stopper,
                maintenance,
            }),
            cleanup,
        }
    }
}

impl<S, M, C> Future for SignalStateMaintenance<S, M, C>
where
    S: Future<Output = ()>,
    M: Future<Output = Result<(), Box<dyn std::error::Error>>>,
    C: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
    type Output = Result<(), Box<dyn std::error::Error>>;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Box<dyn std::error::Error>>> {
        let mut this = self.project();
        if let Some(running) = this.running.as_mut().as_pin_mut() {
            let running_this = running.project();
            if running_this.stopper.poll(cx).is_pending() {
                return running_this.maintenance.poll(cx);
            }
        }
        this.running.set(None);
        this.cleanup.poll(cx)
    }
}

enum MaintenanceAction {
    NoAction,
    Flush,
    Reload(u32),
}

#[resource]
impl Resource for SignalState {
    fn new(
        d: SignalStateDependencies,
        a: SignalStateArgs,
        api: &mut AssemblyRuntime<'_>,
    ) -> Result<Arc<Self>, SignalStateError> {
        let (cipher, initial_state) = match a.bootstrap {
            Some(bootstrap) => {
                let key = ChaCha20Poly1305::generate_key(&mut OsRng);
                let mut f = std::fs::File::create_new(a.encryption_key)?;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                f.write_all(key.as_slice())?;
                f.set_permissions(std::fs::Permissions::from_mode(0o400))?;
                f.sync_all()?;
                drop(f);
                let cipher = ChaCha20Poly1305::new(&key);
                let state = pack_state(&cipher, &bootstrap)?;
                (cipher, Some(state))
            }
            None => {
                let key = std::fs::read(a.encryption_key)?;
                let cipher = ChaCha20Poly1305::new_from_slice(&key)?;
                (cipher, None)
            }
        };
        let shared = Arc::new(Self {
            inner: tokio::sync::RwLock::new(None),
        });
        let shared2 = Arc::clone(&shared);
        let shared3 = Arc::clone(&shared);
        let stopper = api.self_stop();
        let cleanup_bucket = Arc::clone(&d.0);
        let cleanup_cipher = cipher.clone();
        api.set_task(SignalStateMaintenance::new(
            stopper,
            async move {
                let bucket = d.0.as_ref().as_ref();
                let mut seen_version: u32 = 0;
                if let Some(state) = initial_state {
                    log::info!("Setting initial state as 0");
                    bucket.put_object("0", &state).await?;
                    log::info!("Done bootstrap");
                }
                loop {
                    let mut delete_list = Vec::new();
                    let bucket_list =
                        match bucket.list(String::from(""), Some(String::from(""))).await {
                            Ok(l) => l,
                            Err(e) => {
                                log::warn!("Listing bucket: {e}");
                                tokio::time::sleep(Duration::new(30, 0)).await;
                                continue;
                            }
                        };
                    let best_version = bucket_list
                        .into_iter()
                        .flat_map(|entry| {
                            entry
                                .contents
                                .into_iter()
                                .filter_map(|obj| obj.key.parse::<u32>().ok())
                        })
                        .inspect(|v| {
                            if *v < seen_version.saturating_sub(20) {
                                delete_list.push(*v);
                            }
                        })
                        .max();
                    if !delete_list.is_empty() {
                        log::info!("Deleting old state {delete_list:?}");
                        delete_list
                            .into_iter()
                            .map(|v| bucket.delete_object(v.to_string()))
                            .collect::<FuturesUnordered<_>>()
                            .for_each_concurrent(None, |r| async move {
                                if let Err(e) = r {
                                    log::error!("Deleting old state: {e}");
                                }
                            })
                            .await;
                    }
                    let action = match *shared.inner.read().await {
                        None => match best_version {
                            Some(v) => MaintenanceAction::Reload(v),
                            None => {
                                return Err(SignalStateError::NoStateAvailable.into());
                            }
                        },
                        Some(ref inner) => {
                            if inner.dirtied.load(Ordering::Acquire) {
                                MaintenanceAction::Flush
                            } else {
                                match best_version {
                                    Some(v) => {
                                        if inner.version != v {
                                            log::warn!(
                                                "Version mismatch: we have {} but {v} is available",
                                                inner.version
                                            );
                                            MaintenanceAction::Reload(v)
                                        } else {
                                            MaintenanceAction::NoAction
                                        }
                                    }
                                    None => MaintenanceAction::NoAction,
                                }
                            }
                        }
                    };
                    match action {
                        MaintenanceAction::NoAction => (),
                        MaintenanceAction::Flush => {
                            if let Err(e) = shared
                                .inner
                                .write()
                                .await
                                .as_mut()
                                .unwrap()
                                .save(&cipher, bucket)
                                .await
                            {
                                log::error!("Error persisting state: {e}");
                            }
                        }
                        MaintenanceAction::Reload(version) => {
                            let mut inner = shared.inner.write().await;
                            if !inner
                                .as_ref()
                                .map(|inner| inner.dirtied.load(Ordering::Acquire))
                                .unwrap_or(false)
                            {
                                match Inner::load(&cipher, bucket, version).await {
                                    Ok(r) => {
                                        *inner = Some(r);
                                        seen_version = version;
                                    }
                                    Err(e) => {
                                        log::error!("Failed to load state {version}: {e}");
                                    }
                                }
                            }
                        }
                    }
                    tokio::time::sleep(MAINTENANCE_INTERVAL).await;
                }
            },
            async move {
                log::info!("SignalState shutdown requested");
                let mut inner = shared2.inner.write().await;
                log::info!("SignalState shutdown lock acquired");
                match inner.take() {
                    None => {
                        log::info!("SignalState was never loaded");
                    }
                    Some(inner) => {
                        if inner.dirtied.load(Ordering::Acquire) {
                            let state = pack_state(&cleanup_cipher, inner.dir.path())?;
                            let version = inner.version + 1;
                            log::info!("Setting final state as {version}");
                            cleanup_bucket
                                .as_ref()
                                .as_ref()
                                .put_object(version.to_string(), &state)
                                .await?;
                            log::info!("Done cleanup");
                        } else {
                            log::info!("SignalState is not dirty");
                        }
                    }
                }
                Ok(())
            },
        ));
        Ok(shared3)
    }
}
