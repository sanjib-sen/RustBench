//! Sui Issue #828: Sync Mutex in Async Context
//!
//! This reproduces a blocking bug where std::sync::Mutex is used
//! in async code, causing the runtime to block and serialize requests.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/828
//!
//! Note: Modern Rust compilers catch the "hold MutexGuard across await" case
//! at compile time due to !Send. This example demonstrates a subtler version:
//! blocking I/O inside a sync lock in async context.

use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

// State shared across async tasks
struct ServerState {
    request_count: u64,
}

impl ServerState {
    fn new() -> Self {
        Self { request_count: 0 }
    }
}

/// Buggy version: Uses std::sync::Mutex with blocking operations
mod buggy {
    use super::*;
    use std::sync::Mutex;

    pub struct Server {
        state: Arc<Mutex<ServerState>>,
    }

    impl Server {
        pub fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(ServerState::new())),
            }
        }

        /// Handle a request - BUG: performs blocking sleep while holding sync lock
        /// This blocks the entire runtime thread
        pub async fn handle_request(&self, request_id: u64) -> u64 {
            // BUG: Acquire lock then do blocking work
            // This blocks the OS thread, preventing other async tasks from running
            let mut guard = self.state.lock().unwrap();

            // BUG: std::thread::sleep blocks the entire thread!
            // In the original Sui bug, this was blocking I/O (take() operation)
            std::thread::sleep(Duration::from_millis(100));

            guard.request_count += 1;
            let count = guard.request_count;

            println!(
                "[BUGGY] Request {} completed (total: {})",
                request_id, count
            );
            count
        }
    }
}

/// Fixed version: Uses tokio::sync::Mutex with async sleep
mod fixed {
    use super::*;
    use tokio::sync::Mutex;

    pub struct Server {
        state: Arc<Mutex<ServerState>>,
    }

    impl Server {
        pub fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(ServerState::new())),
            }
        }

        /// Handle a request - FIXED: uses async-aware mutex and async sleep
        pub async fn handle_request(&self, request_id: u64) -> u64 {
            // FIXED: tokio::sync::Mutex yields to runtime
            let mut guard = self.state.lock().await;

            // FIXED: async sleep yields to runtime
            tokio::time::sleep(Duration::from_millis(100)).await;

            guard.request_count += 1;
            let count = guard.request_count;

            println!(
                "[FIXED] Request {} completed (total: {})",
                request_id, count
            );
            count
        }
    }
}

async fn run_buggy_test() {
    println!("--- BUGGY VERSION (std::sync::Mutex + blocking sleep) ---\n");

    let server = buggy::Server::new();
    let server = Arc::new(server);

    let start = Instant::now();

    // Spawn 5 concurrent requests
    let mut handles = vec![];
    for i in 0..5 {
        let srv = Arc::clone(&server);
        handles.push(tokio::spawn(async move {
            srv.handle_request(i).await
        }));
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.unwrap();
    }

    let elapsed = start.elapsed();
    println!("\nBuggy version took: {:?}", elapsed);
    println!("Expected ~100ms if concurrent, got ~500ms due to blocking\n");
}

async fn run_fixed_test() {
    println!("--- FIXED VERSION (tokio::sync::Mutex + async sleep) ---\n");

    let server = fixed::Server::new();
    let server = Arc::new(server);

    let start = Instant::now();

    // Spawn 5 concurrent requests
    let mut handles = vec![];
    for i in 0..5 {
        let srv = Arc::clone(&server);
        handles.push(tokio::spawn(async move {
            srv.handle_request(i).await
        }));
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.unwrap();
    }

    let elapsed = start.elapsed();
    println!("\nFixed version took: {:?}", elapsed);
    println!("Should be ~500ms (serialized by mutex, but async-friendly)\n");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #828: Sync Mutex in Async Context ===\n");

    if use_fixed {
        run_fixed_test().await;
    } else {
        run_buggy_test().await;
    }

    if !use_fixed {
        println!("[BUG DEMONSTRATED]");
        println!("Using std::sync::Mutex with blocking operations in async code");
        println!("blocks the entire runtime thread, serializing all tasks.");
        println!("\nRun with --fixed to see proper async mutex behavior.");
    } else {
        println!("[FIXED]");
        println!("Using tokio::sync::Mutex with async operations");
        println!("allows the runtime to schedule other tasks while waiting.");
    }
}
