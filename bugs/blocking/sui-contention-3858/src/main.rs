//! Sui PR #3858: False Contention in Mutex Table
//!
//! This reproduces a performance bug where a fixed-size hash table for locks
//! causes false contention. Unrelated transactions/objects hash to the same
//! slot and block each other unnecessarily.
//!
//! Original PR: https://github.com/MystenLabs/sui/pull/3858

use std::collections::hash_map::DefaultHasher;
use std::env;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

pub type ObjectId = u64;

/// Calculate hash for an object
fn hash_object(id: ObjectId) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// Buggy version - fixed-size lock table causes false contention
mod buggy {
    use super::*;

    const TABLE_SIZE: usize = 4; // Small table = lots of collisions

    pub struct LockTable {
        slots: Vec<Mutex<()>>,
    }

    impl LockTable {
        pub fn new() -> Self {
            let mut slots = Vec::with_capacity(TABLE_SIZE);
            for _ in 0..TABLE_SIZE {
                slots.push(Mutex::new(()));
            }
            Self { slots }
        }

        /// BUG: Fixed-size table causes false contention
        /// Different objects can hash to the same slot!
        pub fn acquire(&self, object_id: ObjectId) -> std::sync::MutexGuard<()> {
            let hash = hash_object(object_id);
            let slot = (hash as usize) % TABLE_SIZE;
            println!(
                "[BUGGY] Object {} -> slot {} (hash collision possible!)",
                object_id, slot
            );
            self.slots[slot].lock().unwrap()
        }
    }

    pub fn process_objects(table: Arc<LockTable>, objects: Vec<ObjectId>, thread_id: usize) -> Duration {
        let start = Instant::now();

        for obj_id in objects {
            let _guard = table.acquire(obj_id);
            // Simulate transaction processing
            thread::sleep(Duration::from_millis(10));
            println!("[BUGGY] Thread {} processed object {}", thread_id, obj_id);
        }

        start.elapsed()
    }
}

/// Fixed version - sharded lock table reduces false contention
mod fixed {
    use super::*;

    const NUM_SHARDS: usize = 16;
    const SHARD_SIZE: usize = 16;

    /// Per-object lock within a shard
    pub struct ShardedLockTable {
        shards: Vec<RwLock<Vec<Mutex<()>>>>,
    }

    impl ShardedLockTable {
        pub fn new() -> Self {
            let mut shards = Vec::with_capacity(NUM_SHARDS);
            for _ in 0..NUM_SHARDS {
                let mut shard = Vec::with_capacity(SHARD_SIZE);
                for _ in 0..SHARD_SIZE {
                    shard.push(Mutex::new(()));
                }
                shards.push(RwLock::new(shard));
            }
            Self { shards }
        }

        /// FIX: Two-level hashing reduces collisions
        pub fn acquire(&self, object_id: ObjectId) -> std::sync::MutexGuard<()> {
            let hash = hash_object(object_id);
            let shard_idx = (hash as usize) % NUM_SHARDS;
            let slot_idx = ((hash >> 16) as usize) % SHARD_SIZE;

            println!(
                "[FIXED] Object {} -> shard {}, slot {} (better distribution)",
                object_id, shard_idx, slot_idx
            );

            let shard = self.shards[shard_idx].read().unwrap();
            // Note: In real code, we'd need to handle this differently
            // For demo, we just show the concept
            unsafe {
                let slot_ptr = &shard[slot_idx] as *const Mutex<()>;
                (*slot_ptr).lock().unwrap()
            }
        }
    }

    pub fn process_objects(table: Arc<ShardedLockTable>, objects: Vec<ObjectId>, thread_id: usize) -> Duration {
        let start = Instant::now();

        for obj_id in objects {
            let _guard = table.acquire(obj_id);
            thread::sleep(Duration::from_millis(10));
            println!("[FIXED] Thread {} processed object {}", thread_id, obj_id);
        }

        start.elapsed()
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui PR #3858: False Contention in Mutex Table ===\n");

    if use_fixed {
        println!("Running FIXED version (sharded lock table)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (fixed-size table with collisions)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let table = Arc::new(buggy::LockTable::new());

    // Create objects that will hash to different slots
    // But with only 4 slots, many will collide!
    let all_objects: Vec<ObjectId> = (1..=8).collect();

    println!("Lock table size: 4 slots");
    println!("Processing {} objects across 2 threads\n", all_objects.len());

    // Show which objects collide
    println!("Object -> Slot mapping:");
    for obj in &all_objects {
        let hash = hash_object(*obj);
        let slot = (hash as usize) % 4;
        println!("  Object {} -> Slot {}", obj, slot);
    }
    println!();

    let objects1: Vec<ObjectId> = all_objects.iter().cloned().filter(|x| x % 2 == 1).collect();
    let objects2: Vec<ObjectId> = all_objects.iter().cloned().filter(|x| x % 2 == 0).collect();

    let table1 = Arc::clone(&table);
    let table2 = Arc::clone(&table);

    let start = Instant::now();

    let t1 = thread::spawn(move || buggy::process_objects(table1, objects1, 1));
    let t2 = thread::spawn(move || buggy::process_objects(table2, objects2, 2));

    let time1 = t1.join().unwrap();
    let time2 = t2.join().unwrap();
    let total = start.elapsed();

    println!("\n=== Results ===");
    println!("[BUG DEMONSTRATED]");
    println!("Thread 1 time: {:?}", time1);
    println!("Thread 2 time: {:?}", time2);
    println!("Total time: {:?}", total);
    println!("\nProblem: Different objects collide on same lock slot!");
    println!("  - Objects with same (hash % 4) block each other");
    println!("  - False contention slows down parallel processing");
    println!("  - Gets worse with more concurrent transactions");
    println!("\nRun with --fixed to see sharded lock table.");
}

fn run_fixed_test() {
    let table = Arc::new(fixed::ShardedLockTable::new());

    let all_objects: Vec<ObjectId> = (1..=8).collect();

    println!("Lock table: 16 shards x 16 slots = 256 possible locks");
    println!("Processing {} objects across 2 threads\n", all_objects.len());

    let objects1: Vec<ObjectId> = all_objects.iter().cloned().filter(|x| x % 2 == 1).collect();
    let objects2: Vec<ObjectId> = all_objects.iter().cloned().filter(|x| x % 2 == 0).collect();

    let table1 = Arc::clone(&table);
    let table2 = Arc::clone(&table);

    let start = Instant::now();

    let t1 = thread::spawn(move || fixed::process_objects(table1, objects1, 1));
    let t2 = thread::spawn(move || fixed::process_objects(table2, objects2, 2));

    let time1 = t1.join().unwrap();
    let time2 = t2.join().unwrap();
    let total = start.elapsed();

    println!("\n=== Results ===");
    println!("[FIXED]");
    println!("Thread 1 time: {:?}", time1);
    println!("Thread 2 time: {:?}", time2);
    println!("Total time: {:?}", total);
    println!("\nImprovement: Sharded table reduces false contention");
    println!("  - 256 possible slots vs 4 in buggy version");
    println!("  - Different objects rarely collide");
    println!("  - Better parallelism under high load");
}
