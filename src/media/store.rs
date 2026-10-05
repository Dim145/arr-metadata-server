//! Where the bytes go: a directory this server serves, or a bucket.
//!
//! One interface over both, from `object_store`, so that everything above it
//! — fetching, serving, sweeping — is written once. A key names its bytes:
//! `<sha256>.<ext>`, filed two levels deep so a directory of a hundred
//! thousand pictures is not one directory.

use std::{ops::Range, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use object_store::{
    Attribute, AttributeValue, Attributes, GetOptions, GetRange, ObjectStore, ObjectStoreExt as _,
    PutOptions, PutPayload, aws::AmazonS3, local::LocalFileSystem, path::Path, signer::Signer as _,
};

use crate::config;

/// The store, as configured.
pub struct Store {
    inner: Arc<dyn ObjectStore>,
    /// The bucket itself, when it is one: the only backend that signs URLs.
    s3: Option<Arc<AmazonS3>>,
    /// Filed under this, in a bucket that holds other things.
    prefix: Option<String>,
    pub backend: config::MediaStorage,
}

/// What a key's bytes are, as found.
pub struct Found {
    pub stream: futures::stream::BoxStream<'static, std::io::Result<Bytes>>,
    /// The whole object's size, whatever range was asked for.
    pub size: u64,
    /// The bytes served: all of them, or the range asked for.
    pub range: Range<u64>,
}

impl Store {
    pub fn open(cfg: &config::Media) -> Result<Option<Self>> {
        match cfg.storage {
            config::MediaStorage::Off => Ok(None),
            config::MediaStorage::Filesystem => {
                std::fs::create_dir_all(&cfg.dir).with_context(|| {
                    format!("could not create the media directory {}", cfg.dir.display())
                })?;
                // Absolute, because the store resolves every key against it
                // and the working directory is nobody's promise.
                let dir = std::fs::canonicalize(&cfg.dir)?;
                let fs = LocalFileSystem::new_with_prefix(&dir)
                    .with_context(|| {
                        format!("could not open the media directory {}", dir.display())
                    })?
                    .with_automatic_cleanup(true);
                Ok(Some(Self {
                    inner: Arc::new(fs),
                    s3: None,
                    prefix: None,
                    backend: cfg.storage,
                }))
            }
            config::MediaStorage::S3 => {
                let s3 = cfg
                    .s3
                    .as_ref()
                    .context("AMS_MEDIA_STORAGE=s3 without its bucket")?;
                let mut builder = object_store::aws::AmazonS3Builder::new()
                    .with_region(&s3.region)
                    .with_bucket_name(&s3.bucket)
                    .with_access_key_id(&s3.access_key)
                    .with_secret_access_key(&s3.secret_key)
                    .with_virtual_hosted_style_request(!s3.path_style)
                    .with_client_options(
                        object_store::ClientOptions::new()
                            .with_allow_http(
                                s3.endpoint
                                    .as_deref()
                                    .is_some_and(|e| e.starts_with("http://")),
                            )
                            .with_timeout(Duration::from_secs(120))
                            .with_connect_timeout(Duration::from_secs(10)),
                    );
                if let Some(endpoint) = &s3.endpoint {
                    builder = builder.with_endpoint(endpoint);
                }
                let store = Arc::new(builder.build().context("could not open the S3 store")?);
                Ok(Some(Self {
                    inner: store.clone(),
                    s3: Some(store),
                    prefix: s3.prefix.clone(),
                    backend: cfg.storage,
                }))
            }
        }
    }

    /// Where a key is filed.
    fn path(&self, key: &str) -> Path {
        let fan = if key.len() >= 4 {
            format!("{}/{}/{key}", &key[0..2], &key[2..4])
        } else {
            key.to_string()
        };
        match &self.prefix {
            Some(prefix) => Path::from(format!("{prefix}/{fan}")),
            None => Path::from(fan),
        }
    }

    /// The key of a filed path, when it is one this store filed.
    fn key_of(&self, path: &Path) -> Option<String> {
        let name = path.parts().next_back()?.as_ref().to_string();
        super::valid_key(&name).then_some(name)
    }

    pub async fn put(&self, key: &str, bytes: Bytes, content_type: &str) -> Result<()> {
        // A bucket keeps a content type and a caching rule with the object,
        // for a reader sent straight to it; a directory has nowhere to put
        // them, and refuses them.
        let mut attributes = Attributes::new();
        if self.s3.is_some() {
            attributes.insert(
                Attribute::ContentType,
                AttributeValue::from(content_type.to_string()),
            );
            attributes.insert(
                Attribute::CacheControl,
                AttributeValue::from("public, max-age=31536000, immutable"),
            );
        }
        self.inner
            .put_opts(
                &self.path(key),
                PutPayload::from_bytes(bytes),
                PutOptions {
                    attributes,
                    ..Default::default()
                },
            )
            .await
            .with_context(|| format!("could not store {key}"))?;
        Ok(())
    }

    /// The bytes under a key, or the part of them asked for. `None` when
    /// the store has no such key.
    pub async fn get(&self, key: &str, range: Option<GetRange>) -> Result<Option<Found>> {
        let options = GetOptions {
            range,
            ..Default::default()
        };
        match self.inner.get_opts(&self.path(key), options).await {
            Ok(result) => {
                let size = result.meta.size;
                let range = result.range.clone();
                let stream = result
                    .into_stream()
                    .map_err(|e| std::io::Error::other(e.to_string()))
                    .boxed();
                Ok(Some(Found {
                    stream,
                    size,
                    range,
                }))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            // A range past the end, mostly: the caller answers as it sees fit.
            Err(object_store::Error::NotImplemented { .. }) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("could not read {key}")),
        }
    }

    /// The whole of a key's bytes.
    pub async fn read(&self, key: &str) -> Result<Option<Bytes>> {
        match self.inner.get(&self.path(key)).await {
            Ok(result) => Ok(Some(result.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("could not read {key}")),
        }
    }

    pub async fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.size(key).await?.is_some())
    }

    /// How big a key's bytes are, when the store has the key.
    pub async fn size(&self, key: &str) -> Result<Option<u64>> {
        Ok(self.head(key).await?.map(|meta| meta.size))
    }

    /// When a key's bytes were last put, when the store has the key: a put
    /// again — the same bytes, wanted again — makes them new.
    pub async fn modified(&self, key: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(self.head(key).await?.map(|meta| meta.last_modified))
    }

    async fn head(&self, key: &str) -> Result<Option<object_store::ObjectMeta>> {
        match self.inner.head(&self.path(key)).await {
            Ok(meta) => Ok(Some(meta)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("could not look for {key}")),
        }
    }

    /// Delete a key; one already gone is no error. Amazon says nothing of
    /// a key that was not there; Garage says `NoSuchKey`, which is the same
    /// thing.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self.inner.delete(&self.path(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) if e.to_string().contains("NoSuchKey") => Ok(()),
            Err(e) => Err(e).with_context(|| format!("could not delete {key}")),
        }
    }

    /// Every key filed, with when it was put there.
    pub async fn list(&self) -> Result<Vec<(String, chrono::DateTime<chrono::Utc>)>> {
        let prefix = self.prefix.as_ref().map(|p| Path::from(p.as_str()));
        let mut stream = self.inner.list(prefix.as_ref());
        let mut keys = Vec::new();
        while let Some(meta) = stream.next().await {
            let meta = meta.context("could not list the media store")?;
            if let Some(key) = self.key_of(&meta.location) {
                keys.push((key, meta.last_modified));
            }
        }
        Ok(keys)
    }

    /// A URL the bucket itself answers for a while, for a reader sent
    /// straight to it — for the method it will ask with, which the signature
    /// covers. `None` for a store that cannot sign.
    pub async fn presign(
        &self,
        key: &str,
        method: axum::http::Method,
        valid_for: Duration,
    ) -> Result<Option<url::Url>> {
        let Some(s3) = &self.s3 else {
            return Ok(None);
        };
        let url = s3
            .signed_url(method, &self.path(key), valid_for)
            .await
            .with_context(|| format!("could not sign a URL for {key}"))?;
        Ok(Some(url))
    }
}
