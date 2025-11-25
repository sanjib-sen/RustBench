//! Fluvio PR #2490: Write Lock Held Across Async IO Operations
//!
//! This reproduces a blocking issue where a write lock guard is held across
//! simulated async IO operations, preventing other threads from making progress.
//!
//! Original PR: https://github.com/infinyon/fluvio/pull/2490
//!
//! Note: We simulate async behavior with threads and sleep since this is a
//! synchronous reproduction. The core issue is the same: holding a lock
//! during a long-running operation starves other requesters.

use std::env;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

/// Represents cached data that requires both reads and writes
pub struct DataCache {
    data: Vec<u8>,
    version: u64,
}

impl DataCache {
    fn new() -> Self {
        Self {
            data: vec![1, 2, 3, 4, 5],
            version: 0,
        }
    }

    fn update(&mut self, new_data: Vec<u8>) {
        self.data = new_data;
        self.version += 1;
    }

    fn get_data(&self) -> &[u8] {
        &self.data
    }

    fn version(&self) -> u64 {
        self.version
    }
}

/// Simulates async IO operation (like network write)
fn simulate_async_io(data: &[u8]) {
    // Under high load, this can take significant time
    println!("    [IO] Writing {} bytes...", data.len());
    thread::sleep(Duration::from_millis(500)); // Simulate slow IO
    println!("    [IO] Write complete");
}

/// Buggy version - holds write lock across async IO
mod buggy {
    use super::*;

    pub struct StreamWriter {
        cache: Arc<RwLock<DataCache>>,
    }

    impl StreamWriter {
        pub fn new() -> Self {
            Self {
                cache: Arc::new(RwLock::new(DataCache::new())),
            }
        }

        /// BUG: Holds write lock during the entire IO operation
        pub fn write_and_persist(&self, new_data: Vec<u8>) {
            println!("[BUGGY] Acquiring write lock...");
            let mut cache = self.cache.write().unwrap();
            println!("[BUGGY] Got write lock, updating cache...");

            cache.update(new_data);

            // BUG: Still holding write lock during slow IO!
            println!("[BUGGY] Persisting to disk (holding lock)...");
            simulate_async_io(cache.get_data());

            println!("[BUGGY] Done, releasing lock");
            // Lock released here when `cache` goes out of scope
        }

        pub fn read_data(&self) -> u64 {
            let start = Instant::now();
            println!("[BUGGY] Reader: trying to acquire read lock...");
            let cache = self.cache.read().unwrap();
            let blocked_ms = start.elapsed().as_millis();
            println!(
                "[BUGGY] Reader: got lock after {}ms, version={}",
                blocked_ms,
                cache.version()
            );
            blocked_ms as u64
        }

        pub fn cache(&self) -> &Arc<RwLock<DataCache>> {
            &self.cache
        }
    }
}

/// Fixed version - releases lock before async IO
mod fixed {
    use super::*;

    pub struct StreamWriter {
        cache: Arc<RwLock<DataCache>>,
    }

    impl StreamWriter {
        pub fn new() -> Self {
            Self {
                cache: Arc::new(RwLock::new(DataCache::new())),
            }
        }

        /// FIX: Release write lock before slow IO operation
        pub fn write_and_persist(&self, new_data: Vec<u8>) {
            // Get the data to persist while holding the lock
            let data_to_persist;

            {
                println!("[FIXED] Acquiring write lock...");
                let mut cache = self.cache.write().unwrap();
                println!("[FIXED] Got write lock, updating cache...");
                cache.update(new_data);

                // Clone the data we need to persist
                data_to_persist = cache.get_data().to_vec();

                println!("[FIXED] Cache updated, releasing lock before IO...");
                // Lock released here!
            }

            // FIX: IO happens OUTSIDE the lock scope
            println!("[FIXED] Persisting to disk (lock released)...");
            simulate_async_io(&data_to_persist);
            println!("[FIXED] Done");
        }

        pub fn read_data(&self) -> u64 {
            let start = Instant::now();
            println!("[FIXED] Reader: trying to acquire read lock...");
            let cache = self.cache.read().unwrap();
            let blocked_ms = start.elapsed().as_millis();
            println!(
                "[FIXED] Reader: got lock after {}ms, version={}",
                blocked_ms,
                cache.version()
            );
            blocked_ms as u64
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Fluvio PR #2490: Write Lock Across Async IO ===\n");

    if use_fixed {
        println!("Running FIXED version (lock released before IO)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (lock held during IO)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let writer = Arc::new(buggy::StreamWriter::new());

    println!("Scenario: Writer updates cache and persists to disk");
    println!("Problem: Lock held during entire IO operation blocks readers\n");

    let writer1 = Arc::clone(&writer);
    let writer2 = Arc::clone(&writer);

    // Spawn writer thread
    let write_handle = thread::spawn(move || {
        writer1.write_and_persist(vec![10, 20, 30, 40, 50]);
    });

    // Small delay to ensure writer gets lock first
    thread::sleep(Duration::from_millis(50));

    // Spawn reader thread - will be blocked!
    let read_handle = thread::spawn(move || {
        writer2.read_data()
    });

    write_handle.join().unwrap();
    let blocked_time = read_handle.join().unwrap();

    println!("\n=== Results ===");
    if blocked_time > 400 {
        println!("[BUG DEMONSTRATED]");
        println!("Reader blocked for {}ms waiting for write lock!", blocked_time);
        println!("\nProblem: Write lock held during 500ms IO operation");
        println!("  - Writer holds lock: acquire -> update -> IO -> release");
        println!("  - Reader blocked: ~500ms waiting for lock");
        println!("\nIn real async code, this causes complete starvation under load.");
        println!("\nRun with --fixed to see lock released before IO.");
    } else {
        println!("Reader not significantly blocked (timing variation)");
        println!("Re-run to trigger the blocking behavior.");
    }
}

fn run_fixed_test() {
    let writer = Arc::new(fixed::StreamWriter::new());

    println!("Scenario: Writer updates cache and persists to disk");
    println!("Fix: Lock released before slow IO operation\n");

    let writer1 = Arc::clone(&writer);
    let writer2 = Arc::clone(&writer);

    let write_handle = thread::spawn(move || {
        writer1.write_and_persist(vec![10, 20, 30, 40, 50]);
    });

    // Small delay to ensure writer starts first
    thread::sleep(Duration::from_millis(50));

    let read_handle = thread::spawn(move || {
        writer2.read_data()
    });

    write_handle.join().unwrap();
    let blocked_time = read_handle.join().unwrap();

    println!("\n=== Results ===");
    if blocked_time < 100 {
        println!("[FIXED]");
        println!("Reader blocked for only {}ms!", blocked_time);
        println!("\nFix: Lock released before IO operation");
        println!("  - Writer: acquire -> update -> release -> IO");
        println!("  - Reader: can acquire lock during IO phase");
        println!("\nNo starvation, concurrent access works properly.");
    } else {
        println!("Reader still blocked for {}ms", blocked_time);
        println!("This may be due to timing; the fix reduces typical blocking.");
    }
}
