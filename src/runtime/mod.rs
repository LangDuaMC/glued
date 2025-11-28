use crate::types::Update;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

pub mod docker;
pub use docker::DockerRuntime;

#[async_trait]
pub trait ContainerRuntime {
    /// Start monitoring the runtime for container changes.
    /// Updates should be sent to the provided channel.
    async fn monitor(&self, update_tx: mpsc::Sender<Update>) -> Result<()>;
}
