//! SurrealDB Issue #3987: RwLock Contention Deadlock
//!
//! This reproduces a deadlock that occurs when multiple async tasks
//! contend heavily on a RwLock, especially with writer starvation.
//!
//! Original bug: https://github.com/surrealdb/surrealdb/issues/3987

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;

/// Simulates the WEBSOCKETS global registry
type ConnectionRegistry = Arc<RwLock<HashMap<u64, String>>>;

/// Buggy version: Uses blocking .read().await under contention
mod buggy {
    use super::*;

    pub struct ConnectionManager {
        connections: ConnectionRegistry,
    }

    impl ConnectionManager {
        pub fn new() -> Self {
            Self {
                connections: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        /// Check if connection exists - BUG: blocking read
        pub async fn check_connection(&self, id: u64) -> bool {
            // BUG: Under high contention, this .read().await can contribute
            // to deadlock due to writer starvation or priority inversion
            let guard = self.connections.read().await;
            guard.contains_key(&id)
        }

        /// Add a new connection - needs write lock
        pub async fn add_connection(&self, id: u64, info: String) {
            // Writer may starve if readers keep holding the lock
            let mut guard = self.connections.write().await;
            guard.insert(id, info);
        }

        /// Remove a connection - needs write lock
        pub async fn remove_connection(&self, id: u64) {
            let mut guard = self.connections.write().await;
            guard.remove(&id);
        }

        /// Simulate live query notification - reads all connections
        pub async fn notify_all(&self, _message: &str) {
            let guard = self.connections.read().await;
            // Hold read lock while "sending" to all connections
            for (_id, _conn) in guard.iter() {
                // Simulate sending notification
                tokio::time::sleep(Duration::from_micros(100)).await;
            }
        }

        pub fn get_registry(&self) -> ConnectionRegistry {
            Arc::clone(&self.connections)
        }
    }
}

/// Fixed version: Uses try_read with backoff
mod fixed {
    use super::*;

    pub struct ConnectionManager {
        connections: ConnectionRegistry,
    }

    impl ConnectionManager {
        pub fn new() -> Self {
            Self {
                connections: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        /// Check if connection exists - FIXED: non-blocking with retry
        pub async fn check_connection(&self, id: u64) -> bool {
            // FIX: Use try_read with exponential backoff
            let mut delay = Duration::from_micros(100);
            let max_delay = Duration::from_millis(10);

            loop {
                match self.connections.try_read() {
                    Ok(guard) => return guard.contains_key(&id),
                    Err(_) => {
                        tokio::time::sleep(delay).await;
                        delay = std::cmp::min(delay * 2, max_delay);
                    }
                }
            }
        }

        pub async fn add_connection(&self, id: u64, info: String) {
            // Also use try_write with backoff for writers
            let mut delay = Duration::from_micros(100);
            let max_delay = Duration::from_millis(10);

            loop {
                match self.connections.try_write() {
                    Ok(mut guard) => {
                        guard.insert(id, info);
                        return;
                    }
                    Err(_) => {
                        tokio::time::sleep(delay).await;
                        delay = std::cmp::min(delay * 2, max_delay);
                    }
                }
            }
        }

        pub async fn remove_connection(&self, id: u64) {
            let mut delay = Duration::from_micros(100);
            loop {
                match self.connections.try_write() {
                    Ok(mut guard) => {
                        guard.remove(&id);
                        return;
                    }
                    Err(_) => {
                        tokio::time::sleep(delay).await;
                        delay = std::cmp::min(delay * 2, Duration::from_millis(10));
                    }
                }
            }
        }

        pub async fn notify_all(&self, _message: &str) {
            // Take a snapshot instead of holding lock during notification
            let connections: Vec<(u64, String)> = {
                match self.connections.try_read() {
                    Ok(guard) => guard.iter().map(|(k, v)| (*k, v.clone())).collect(),
                    Err(_) => return, // Skip this notification cycle
                }
            };

            for (_id, _conn) in connections {
                tokio::time::sleep(Duration::from_micros(100)).await;
            }
        }

        pub fn get_registry(&self) -> ConnectionRegistry {
            Arc::clone(&self.connections)
        }
    }
}

async fn run_buggy_test() {
    println!("--- BUGGY VERSION (blocking .read().await) ---\n");

    let manager = Arc::new(buggy::ConnectionManager::new());
    let start = Instant::now();
    let timeout = Duration::from_secs(5);

    // Pre-populate some connections
    for i in 0..10 {
        manager.add_connection(i, format!("conn_{}", i)).await;
    }

    let mut handles = vec![];

    // Spawn many reader tasks (checking connections)
    for i in 0..20 {
        let mgr = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for j in 0..100 {
                mgr.check_connection((i * 100 + j) % 20).await;
                tokio::task::yield_now().await;
            }
        }));
    }

    // Spawn writer tasks (add/remove connections)
    for i in 0..5 {
        let mgr = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for j in 0..20 {
                let id = 100 + i * 20 + j;
                mgr.add_connection(id, format!("new_conn_{}", id)).await;
                tokio::time::sleep(Duration::from_micros(500)).await;
                mgr.remove_connection(id).await;
            }
        }));
    }

    // Spawn notification tasks (long reads)
    for _ in 0..3 {
        let mgr = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                mgr.notify_all("update").await;
            }
        }));
    }

    // Wait with timeout
    let results = tokio::time::timeout(timeout, async {
        for handle in handles {
            let _ = handle.await;
        }
    })
    .await;

    let elapsed = start.elapsed();

    match results {
        Ok(_) => {
            println!("Completed in {:?}", elapsed);
            println!("\n[NOTE] No deadlock this run (timing-dependent)");
        }
        Err(_) => {
            println!("TIMEOUT after {:?}!", timeout);
            println!("\n[BUG DEMONSTRATED]");
            println!("Tasks deadlocked or starved due to RwLock contention.");
        }
    }
}

async fn run_fixed_test() {
    println!("--- FIXED VERSION (try_read with backoff) ---\n");

    let manager = Arc::new(fixed::ConnectionManager::new());
    let start = Instant::now();
    let timeout = Duration::from_secs(5);

    for i in 0..10 {
        manager.add_connection(i, format!("conn_{}", i)).await;
    }

    let mut handles = vec![];

    for i in 0..20 {
        let mgr = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for j in 0..100 {
                mgr.check_connection((i * 100 + j) % 20).await;
                tokio::task::yield_now().await;
            }
        }));
    }

    for i in 0..5 {
        let mgr = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for j in 0..20 {
                let id = 100 + i * 20 + j;
                mgr.add_connection(id, format!("new_conn_{}", id)).await;
                tokio::time::sleep(Duration::from_micros(500)).await;
                mgr.remove_connection(id).await;
            }
        }));
    }

    for _ in 0..3 {
        let mgr = Arc::clone(&manager);
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                mgr.notify_all("update").await;
            }
        }));
    }

    let results = tokio::time::timeout(timeout, async {
        for handle in handles {
            let _ = handle.await;
        }
    })
    .await;

    let elapsed = start.elapsed();

    match results {
        Ok(_) => {
            println!("Completed in {:?}", elapsed);
            println!("\n[FIXED]");
            println!("Non-blocking try_read with backoff prevents deadlock.");
        }
        Err(_) => {
            println!("TIMEOUT (unexpected) after {:?}", timeout);
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== SurrealDB Issue #3987: RwLock Contention Deadlock ===\n");

    if use_fixed {
        run_fixed_test().await;
    } else {
        run_buggy_test().await;
    }

    if !use_fixed {
        println!("\nRun with --fixed to see non-blocking version.");
    }
}
