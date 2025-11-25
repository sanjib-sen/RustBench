//! Reth Issue #10842: Lock Ordering Deadlock
//!
//! This reproduces a classic lock ordering deadlock where two locks
//! (numbers and blocks) are acquired in inconsistent order across
//! different operations, leading to potential deadlock.
//!
//! Original fix: https://github.com/paradigmxyz/reth/pull/10842

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

pub type BlockNumber = u64;
pub type BlockHash = String;

#[derive(Debug, Clone)]
pub struct Block {
    number: BlockNumber,
    hash: BlockHash,
    data: String,
}

/// Chain state with two locks
pub struct ChainState {
    numbers: RwLock<HashMap<BlockHash, BlockNumber>>,
    blocks: RwLock<HashMap<BlockNumber, Block>>,
}

impl ChainState {
    fn new() -> Self {
        let mut numbers = HashMap::new();
        let mut blocks = HashMap::new();

        // Add some initial blocks
        for i in 0..5 {
            let block = Block {
                number: i,
                hash: format!("hash_{}", i),
                data: format!("block_data_{}", i),
            };
            numbers.insert(block.hash.clone(), block.number);
            blocks.insert(block.number, block);
        }

        Self {
            numbers: RwLock::new(numbers),
            blocks: RwLock::new(blocks),
        }
    }
}

/// Buggy implementation - inconsistent lock order
mod buggy {
    use super::*;

    pub struct ChainStateManager {
        state: Arc<ChainState>,
    }

    impl ChainStateManager {
        pub fn new(state: Arc<ChainState>) -> Self {
            Self { state }
        }

        /// BUG: Acquires numbers lock first, then blocks lock
        pub fn read_operation(&self, hash: &str) -> Option<Block> {
            println!("[BUGGY] read_operation: acquiring numbers lock...");
            let numbers = self.state.numbers.read().unwrap();

            thread::sleep(Duration::from_millis(50)); // Simulate work

            if let Some(&block_number) = numbers.get(hash) {
                println!("[BUGGY] read_operation: acquiring blocks lock...");
                let blocks = self.state.blocks.read().unwrap();
                blocks.get(&block_number).cloned()
            } else {
                None
            }
        }

        /// BUG: Acquires blocks lock first, then numbers lock (WRONG ORDER!)
        pub fn write_operation(&self, block: Block) {
            println!("[BUGGY] write_operation: acquiring blocks lock...");
            let mut blocks = self.state.blocks.write().unwrap();

            thread::sleep(Duration::from_millis(50)); // Simulate work

            println!("[BUGGY] write_operation: acquiring numbers lock...");
            let mut numbers = self.state.numbers.write().unwrap();

            numbers.insert(block.hash.clone(), block.number);
            blocks.insert(block.number, block);

            println!("[BUGGY] write_operation: completed");
        }

        /// Another read operation using the SAME (correct) order
        pub fn another_read(&self, number: BlockNumber) -> Option<Block> {
            println!("[BUGGY] another_read: acquiring numbers lock...");
            let _numbers = self.state.numbers.read().unwrap();

            thread::sleep(Duration::from_millis(50)); // Simulate work

            println!("[BUGGY] another_read: acquiring blocks lock...");
            let blocks = self.state.blocks.read().unwrap();
            blocks.get(&number).cloned()
        }
    }
}

/// Fixed implementation - consistent lock order
mod fixed {
    use super::*;

    pub struct ChainStateManager {
        state: Arc<ChainState>,
    }

    impl ChainStateManager {
        pub fn new(state: Arc<ChainState>) -> Self {
            Self { state }
        }

        /// FIX: Always acquire numbers lock first
        pub fn read_operation(&self, hash: &str) -> Option<Block> {
            println!("[FIXED] read_operation: acquiring numbers lock...");
            let numbers = self.state.numbers.read().unwrap();

            if let Some(&block_number) = numbers.get(hash) {
                println!("[FIXED] read_operation: acquiring blocks lock...");
                let blocks = self.state.blocks.read().unwrap();
                blocks.get(&block_number).cloned()
            } else {
                None
            }
        }

        /// FIX: Also acquire numbers lock first (consistent order!)
        pub fn write_operation(&self, block: Block) {
            println!("[FIXED] write_operation: acquiring numbers lock first...");
            let mut numbers = self.state.numbers.write().unwrap();

            println!("[FIXED] write_operation: acquiring blocks lock...");
            let mut blocks = self.state.blocks.write().unwrap();

            numbers.insert(block.hash.clone(), block.number);
            blocks.insert(block.number, block);

            println!("[FIXED] write_operation: completed");
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Reth Issue #10842: Lock Ordering Deadlock ===\n");

    if use_fixed {
        println!("Running FIXED version (consistent lock order)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (inconsistent lock order)...\n");
        println!("NOTE: This may deadlock! Kill with Ctrl+C if it hangs.\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let state = Arc::new(ChainState::new());
    let manager = Arc::new(buggy::ChainStateManager::new(Arc::clone(&state)));

    println!("Starting two threads with conflicting lock orders...\n");

    // Thread 1: read_operation (numbers -> blocks)
    let manager1 = Arc::clone(&manager);
    let handle1 = thread::spawn(move || {
        println!("[Thread 1] Starting read_operation");
        manager1.read_operation("hash_0");
        println!("[Thread 1] Completed read_operation");
    });

    thread::sleep(Duration::from_millis(10)); // Let thread 1 start

    // Thread 2: write_operation (blocks -> numbers) - WRONG ORDER!
    let manager2 = Arc::clone(&manager);
    let handle2 = thread::spawn(move || {
        println!("[Thread 2] Starting write_operation");
        let new_block = Block {
            number: 100,
            hash: "hash_100".to_string(),
            data: "new_data".to_string(),
        };
        manager2.write_operation(new_block);
        println!("[Thread 2] Completed write_operation");
    });

    println!("Waiting for threads to complete (may deadlock)...\n");

    // Try to join with timeout
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(3);

    let mut completed = false;
    loop {
        if handle1.is_finished() && handle2.is_finished() {
            let _ = handle1.join();
            let _ = handle2.join();
            completed = true;
            break;
        }

        if start.elapsed() > timeout {
            println!("\n=== Results ===");
            println!("[DEADLOCK DETECTED]");
            println!("Threads did not complete within 3 seconds!");
            println!("\nDeadlock scenario:");
            println!("  Thread 1: holds numbers(read), waiting for blocks(read)");
            println!("  Thread 2: holds blocks(write), waiting for numbers(write)");
            println!("\nClassic lock ordering deadlock!");
            println!("\nRun with --fixed to see consistent lock ordering.");
            std::process::exit(1);
        }

        thread::sleep(Duration::from_millis(100));
    }

    if completed {
        println!("\n=== Results ===");
        println!("Threads completed successfully this time.");
        println!("(Deadlock is timing-dependent - may not always occur)");
        println!("\nRun with --fixed to see proper lock ordering.");
    }
}

fn run_fixed_test() {
    let state = Arc::new(ChainState::new());
    let manager = Arc::new(fixed::ChainStateManager::new(Arc::clone(&state)));

    println!("Starting two threads with consistent lock order...\n");

    let manager1 = Arc::clone(&manager);
    let handle1 = thread::spawn(move || {
        println!("[Thread 1] Starting read_operation");
        manager1.read_operation("hash_0");
        println!("[Thread 1] Completed read_operation");
    });

    thread::sleep(Duration::from_millis(10));

    let manager2 = Arc::clone(&manager);
    let handle2 = thread::spawn(move || {
        println!("[Thread 2] Starting write_operation");
        let new_block = Block {
            number: 100,
            hash: "hash_100".to_string(),
            data: "new_data".to_string(),
        };
        manager2.write_operation(new_block);
        println!("[Thread 2] Completed write_operation");
    });

    handle1.join().unwrap();
    handle2.join().unwrap();

    println!("\n=== Results ===");
    println!("[FIXED]");
    println!("Both threads completed successfully!");
    println!("Consistent lock order (numbers -> blocks) prevents deadlock.");
}
