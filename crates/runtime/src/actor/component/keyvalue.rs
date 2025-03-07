use super::{Ctx, Instance, TableResult};

use crate::capability::keyvalue::{atomic, readwrite, types, wasi_cloud_error};
use crate::capability::{KeyValueAtomic, KeyValueReadWrite};
use crate::io::AsyncVec;

use std::sync::Arc;

use anyhow::{anyhow, ensure, Context};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::preview2::{self, HostOutputStream, InputStream};

impl Instance {
    /// Set [`KeyValueAtomic`] handler for this [Instance].
    pub fn keyvalue_atomic(
        &mut self,
        keyvalue_atomic: Arc<dyn KeyValueAtomic + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_keyvalue_atomic(keyvalue_atomic);
        self
    }

    /// Set [`KeyValueReadWrite`] handler for this [Instance].
    pub fn keyvalue_readwrite(
        &mut self,
        keyvalue_readwrite: Arc<dyn KeyValueReadWrite + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut()
            .replace_keyvalue_readwrite(keyvalue_readwrite);
        self
    }
}

trait TableKeyValueExt {
    fn get_bucket(&self, bucket: types::Bucket) -> TableResult<&String>;
    fn delete_incoming_value(
        &mut self,
        stream: types::IncomingValue,
    ) -> TableResult<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>;
    fn get_outgoing_value(&self, stream: types::OutgoingValue) -> TableResult<&AsyncVec>;
    fn push_error(&mut self, error: anyhow::Error) -> TableResult<wasi_cloud_error::Error>;
}

impl TableKeyValueExt for preview2::Table {
    fn get_bucket(&self, bucket: types::Bucket) -> TableResult<&String> {
        self.get(&Resource::new_borrow(bucket))
    }

    fn delete_incoming_value(
        &mut self,
        stream: types::IncomingValue,
    ) -> TableResult<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        self.delete(Resource::new_own(stream))
    }

    fn get_outgoing_value(&self, stream: types::OutgoingValue) -> TableResult<&AsyncVec> {
        self.get(&Resource::new_borrow(stream))
    }

    fn push_error(&mut self, error: anyhow::Error) -> TableResult<wasi_cloud_error::Error> {
        let res = self.push(error)?;
        Ok(res.rep())
    }
}

type Result<T, E = types::Error> = core::result::Result<T, E>;

#[async_trait]
impl atomic::Host for Ctx {
    #[instrument]
    async fn increment(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
        delta: u64,
    ) -> anyhow::Result<Result<u64>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.increment(bucket, key, delta).await {
            Ok(new) => Ok(Ok(new)),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn compare_and_swap(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
        old: u64,
        new: u64,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.compare_and_swap(bucket, key, old, new).await {
            Ok(changed) => Ok(Ok(changed)),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }
}

#[async_trait]
impl readwrite::Host for Ctx {
    #[instrument]
    async fn get(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
    ) -> anyhow::Result<Result<types::IncomingValue>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.get(bucket, key).await {
            Ok((stream, size)) => {
                let value = self
                    .table
                    .push((stream, size))
                    .context("failed to push stream and size")?;
                Ok(Ok(value.rep()))
            }
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn set(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
        outgoing_value: types::OutgoingValue,
    ) -> anyhow::Result<Result<()>> {
        let mut stream = self
            .table
            .get_outgoing_value(outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        stream.rewind().await.context("failed to rewind stream")?;
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.set(bucket, key, Box::new(stream)).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn delete(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.delete(bucket, key).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn exists(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.exists(bucket, key).await {
            Ok(true) => Ok(Ok(true)),
            Ok(false) => {
                // NOTE: This is required until
                // https://github.com/WebAssembly/wasi-keyvalue/pull/18 is merged
                let err = self
                    .table
                    .push_error(anyhow!("key does not exist"))
                    .context("failed to push error")?;
                Ok(Err(err))
            }
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }
}

#[async_trait]
impl types::Host for Ctx {
    #[instrument]
    async fn drop_bucket(&mut self, bucket: types::Bucket) -> anyhow::Result<()> {
        let _: String = self
            .table
            .delete(Resource::new_own(bucket))
            .context("failed to delete bucket")?;
        Ok(())
    }

    #[instrument]
    async fn open_bucket(&mut self, name: String) -> anyhow::Result<Result<types::Bucket>> {
        let bucket = self.table.push(name).context("failed to open bucket")?;
        Ok(Ok(bucket.rep()))
    }

    #[instrument]
    async fn drop_outgoing_value(
        &mut self,
        outgoing_value: types::OutgoingValue,
    ) -> anyhow::Result<()> {
        let _: AsyncVec = self
            .table
            .delete(Resource::new_own(outgoing_value))
            .context("failed to delete outgoing value")?;
        Ok(())
    }

    #[instrument]
    async fn new_outgoing_value(&mut self) -> anyhow::Result<types::OutgoingValue> {
        let value = self
            .table
            .push(AsyncVec::default())
            .context("failed to push outgoing value")?;
        Ok(value.rep())
    }

    #[instrument]
    async fn outgoing_value_write_body_sync(
        &mut self,
        outgoing_value: types::OutgoingValue,
        body: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        let mut stream = self
            .table
            .get_outgoing_value(outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        stream
            .write_all(&body)
            .await
            .context("failed to write body")?;
        Ok(Ok(()))
    }

    #[instrument]
    async fn outgoing_value_write_body_async(
        &mut self,
        outgoing_value: types::OutgoingValue,
    ) -> anyhow::Result<Result<Resource<Box<dyn HostOutputStream>>>> {
        let stream = self
            .table
            .get_outgoing_value(outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        let stream: Box<dyn HostOutputStream> = Box::new(AsyncWriteStream::new(1 << 16, stream));
        let stream = self
            .table
            .push(stream)
            .context("failed to push output stream")?;
        Ok(Ok(stream))
    }

    #[instrument]
    async fn drop_incoming_value(
        &mut self,
        incoming_value: types::IncomingValue,
    ) -> anyhow::Result<()> {
        self.table
            .delete_incoming_value(incoming_value)
            .context("failed to delete incoming value")?;
        Ok(())
    }

    #[instrument]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: types::IncomingValue,
    ) -> anyhow::Result<Result<types::IncomingValueSyncBody>> {
        let (stream, size) = self
            .table
            .delete_incoming_value(incoming_value)
            .context("failed to delete incoming value")?;
        let mut stream = stream.take(size);
        let size = size.try_into().context("size does not fit in `usize`")?;
        let mut buf = Vec::with_capacity(size);
        match stream.read_to_end(&mut buf).await {
            Ok(n) => {
                ensure!(n == size);
                Ok(Ok(buf))
            }
            Err(err) => {
                let err = self
                    .table
                    .push_error(anyhow!(err).context("failed to read stream"))
                    .context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: types::IncomingValue,
    ) -> anyhow::Result<Result<Resource<InputStream>>> {
        let (stream, _) = self
            .table
            .delete_incoming_value(incoming_value)
            .context("failed to delete incoming value")?;
        let stream = self
            .table
            .push(InputStream::Host(Box::new(AsyncReadStream::new(stream))))
            .context("failed to push input stream")?;
        Ok(Ok(stream))
    }

    #[instrument]
    async fn size(&mut self, incoming_value: types::IncomingValue) -> anyhow::Result<u64> {
        let (_, size): &(Box<dyn AsyncRead + Sync + Send + Unpin>, _) = self
            .table
            .get(&Resource::new_borrow(incoming_value))
            .context("failed to get incoming value")?;
        Ok(*size)
    }
}

#[async_trait]
impl wasi_cloud_error::Host for Ctx {
    #[instrument]
    async fn drop_error(&mut self, error: wasi_cloud_error::Error) -> anyhow::Result<()> {
        let _: anyhow::Error = self
            .table
            .delete(Resource::new_own(error))
            .context("failed to delete error")?;
        Ok(())
    }

    #[instrument]
    async fn trace(&mut self, error: wasi_cloud_error::Error) -> anyhow::Result<String> {
        self.table
            .get(&Resource::new_borrow(error))
            .context("failed to get error")
            .map(|err: &anyhow::Error| format!("{err:#}"))
    }
}
