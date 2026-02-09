//! Test utilities and server harness for Sluice tests.
//!
//! Provides:
//! - In-process test server setup
//! - gRPC client helpers
//! - Test database fixtures

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;

use sluice_proto::sluice::v1::sluice_client::SluiceClient;
use sluice_proto::sluice::v1::sluice_server::SluiceServer;
use sluice_server::config::Config;
use sluice_server::flow::notify::NotificationBus;
use sluice_server::server::ServerState;
use sluice_server::service::{ConnectionRegistry, SluiceService};
use sluice_server::storage::reader::ReaderPool;
use sluice_server::storage::writer::Writer;
use tonic::transport::{Channel, Server};

/// Test fixture that manages a temporary database directory.
///
/// The directory is automatically cleaned up when the fixture is dropped.
pub struct TestFixture {
    /// Temporary directory for test database
    pub temp_dir: TempDir,
    /// Path to the database file
    pub db_path: PathBuf,
}

impl TestFixture {
    /// Create a new test fixture with a temporary database directory.
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        Self { temp_dir, db_path }
    }

    /// Get the database path as a string.
    pub fn db_path_str(&self) -> &str {
        self.db_path.to_str().expect("invalid path")
    }
}

impl Default for TestFixture {
    fn default() -> Self {
        Self::new()
    }
}

/// Test server that runs in-process.
pub struct TestServer {
    /// The address the server is listening on
    pub addr: SocketAddr,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Server task handle
    server_task: tokio::task::JoinHandle<()>,
    /// Fixture for cleanup (optional if using external data_dir)
    _fixture: Option<TestFixture>,
}

impl TestServer {
    /// Start a new test server on a random available port.
    pub async fn start() -> Self {
        Self::start_with_config(Config::default()).await
    }

    /// Start a new test server with a specific configuration.
    ///
    /// Note: If `config.data_dir` is a valid path, it will be used as-is.
    /// Otherwise, a temporary directory is created.
    pub async fn start_with_config(mut config: Config) -> Self {
        // Only create fixture if data_dir is the default
        let fixture = if config.data_dir.as_os_str().is_empty()
            || config.data_dir.as_path() == std::path::Path::new("./data")
        {
            let f = TestFixture::new();
            config.data_dir = f.temp_dir.path().to_path_buf();
            Some(f)
        } else {
            None
        };

        // Find an available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind");
        let addr = listener.local_addr().expect("failed to get addr");
        drop(listener);

        config.host = "127.0.0.1".to_string();
        config.port = addr.port();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Create notification bus
        let notify_bus = NotificationBus::new(config.notify_channel_size);

        // Spawn writer thread
        let writer = Writer::spawn(
            config.data_dir.join("sluice.db"),
            notify_bus.clone(),
            config.write_channel_size,
            sluice_server::storage::batch::BatchConfig::from_config(1, 1),
            100, // WAL checkpoint pages
        )
        .expect("failed to spawn writer");
        let writer_handle = writer.handle();

        // Create reader pool
        let reader_pool =
            ReaderPool::new(config.data_dir.join("sluice.db"), config.reader_pool_size)
                .expect("failed to create reader pool");

        // Create shared state
        let state = Arc::new(ServerState {
            writer: writer_handle.clone(),
            reader_pool,
            notify_bus,
            connection_registry: ConnectionRegistry::new(),
        });

        // Create service
        let service = SluiceService::new(state);

        let mut shutdown_rx_clone = shutdown_rx.clone();

        // Spawn server task
        let server_task = tokio::spawn(async move {
            Server::builder()
                .add_service(SluiceServer::new(service))
                .serve_with_shutdown(addr, async move {
                    let _ = shutdown_rx_clone.changed().await;
                })
                .await
                .expect("server error");

            // Shutdown writer
            writer_handle
                .shutdown()
                .await
                .expect("writer shutdown failed");
            writer.join().expect("writer join failed");
        });

        // Wait for server to be ready
        tokio::time::sleep(Duration::from_millis(50)).await;

        Self {
            addr,
            shutdown_tx,
            server_task,
            _fixture: fixture,
        }
    }

    /// Get a client connected to this test server.
    pub async fn client(&self) -> SluiceClient<Channel> {
        let endpoint = format!("http://{}", self.addr);
        SluiceClient::connect(endpoint)
            .await
            .expect("failed to connect")
    }

    /// Shutdown the test server.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.server_task.await;
    }
}

/// Wait for a condition to become true with timeout.
///
/// # Arguments
///
/// * `timeout` - Maximum time to wait
/// * `condition` - Closure that returns true when condition is met
///
/// # Returns
///
/// `true` if condition was met, `false` if timeout expired
#[allow(dead_code)]
pub async fn wait_for<F>(timeout: std::time::Duration, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixture_creates_temp_dir() {
        let fixture = TestFixture::new();
        assert!(fixture.temp_dir.path().exists());
        assert!(fixture.db_path_str().contains("test.db"));
    }
}
