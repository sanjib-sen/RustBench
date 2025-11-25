//! Sui Issue #303: Non-Atomic Read-Modify-Write Race (Lost Update)
//!
//! This reproduces a classic atomicity violation where concurrent
//! read-modify-write operations on the pending_orders table cause
//! updates to be lost, leading to double-spending vulnerabilities.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/303

use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct OrderId(String);

#[derive(Debug, Clone)]
pub struct Order {
    id: OrderId,
    amount: u64,
}

/// Buggy client API - non-atomic read-modify-write
mod buggy {
    use super::*;

    pub struct ClientAPI {
        // BUG: Using RwLock but doing non-atomic read-modify-write
        pending_orders: RwLock<HashMap<String, u64>>,
        lost_updates: AtomicU64,
    }

    impl ClientAPI {
        pub fn new() -> Self {
            Self {
                pending_orders: RwLock::new(HashMap::new()),
                lost_updates: AtomicU64::new(0),
            }
        }

        /// BUG: Non-atomic read-modify-write sequence
        pub fn add_pending_order(&self, account: &str, amount: u64) {
            // Step 1: Read current value
            let current = {
                let orders = self.pending_orders.read().unwrap();
                *orders.get(account).unwrap_or(&0)
            };
            // Lock is released here!

            // BUG: Race window! Another thread can modify the value here
            thread::sleep(Duration::from_micros(10)); // Simulate processing

            // Step 2: Compute new value
            let new_value = current + amount;

            // Step 3: Write new value
            {
                let mut orders = self.pending_orders.write().unwrap();
                orders.insert(account.to_string(), new_value);
            }

            println!(
                "[BUGGY] Added {} to account '{}' (read: {}, wrote: {})",
                amount, account, current, new_value
            );
        }

        pub fn get_pending(&self, account: &str) -> u64 {
            let orders = self.pending_orders.read().unwrap();
            *orders.get(account).unwrap_or(&0)
        }

        /// Check for lost updates
        pub fn check_lost_updates(&self, account: &str, expected: u64) -> bool {
            let actual = self.get_pending(account);
            if actual < expected {
                let lost = expected - actual;
                self.lost_updates.fetch_add(lost, Ordering::SeqCst);
                println!(
                    "[BUGGY] LOST UPDATE! Account '{}': expected {}, actual {} (lost: {})",
                    account, expected, actual, lost
                );
                true
            } else {
                false
            }
        }

        pub fn get_lost_updates(&self) -> u64 {
            self.lost_updates.load(Ordering::SeqCst)
        }
    }
}

/// Fixed client API - atomic operations
mod fixed {
    use super::*;

    pub struct ClientAPI {
        // FIX: Keep write lock during entire read-modify-write sequence
        pending_orders: Mutex<HashMap<String, u64>>,
    }

    impl ClientAPI {
        pub fn new() -> Self {
            Self {
                pending_orders: Mutex::new(HashMap::new()),
            }
        }

        /// FIX: Atomic read-modify-write with single lock acquisition
        pub fn add_pending_order(&self, account: &str, amount: u64) {
            let mut orders = self.pending_orders.lock().unwrap();

            // Perform read-modify-write atomically under lock
            let current = *orders.get(account).unwrap_or(&0);
            let new_value = current + amount;
            orders.insert(account.to_string(), new_value);

            println!(
                "[FIXED] Added {} to account '{}' (read: {}, wrote: {})",
                amount, account, current, new_value
            );
        }

        pub fn get_pending(&self, account: &str) -> u64 {
            let orders = self.pending_orders.lock().unwrap();
            *orders.get(account).unwrap_or(&0)
        }
    }
}

/// Alternative fix using atomic types
mod fixed_atomic {
    use super::*;
    use std::sync::atomic::AtomicU64;

    pub struct ClientAPI {
        // Alternative FIX: Use atomic operations directly
        pending_orders: RwLock<HashMap<String, AtomicU64>>,
    }

    impl ClientAPI {
        pub fn new() -> Self {
            Self {
                pending_orders: RwLock::new(HashMap::new()),
            }
        }

        /// FIX: Use fetch_add for atomic increment
        pub fn add_pending_order(&self, account: &str, amount: u64) {
            let orders = self.pending_orders.read().unwrap();

            // Get or create atomic counter for this account
            if !orders.contains_key(account) {
                drop(orders);
                let mut orders_mut = self.pending_orders.write().unwrap();
                orders_mut
                    .entry(account.to_string())
                    .or_insert(AtomicU64::new(0));
                drop(orders_mut);
            }

            let orders = self.pending_orders.read().unwrap();
            let counter = orders.get(account).unwrap();
            let new_value = counter.fetch_add(amount, Ordering::SeqCst) + amount;

            println!(
                "[FIXED-ATOMIC] Added {} to account '{}' (new value: {})",
                amount, account, new_value
            );
        }

        pub fn get_pending(&self, account: &str) -> u64 {
            let orders = self.pending_orders.read().unwrap();
            orders
                .get(account)
                .map(|v| v.load(Ordering::SeqCst))
                .unwrap_or(0)
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");
    let use_atomic = args.iter().any(|arg| arg == "--atomic");

    println!("=== Sui Issue #303: Non-Atomic Read-Modify-Write (Lost Update) ===\n");

    if use_atomic {
        println!("Running FIXED-ATOMIC version (atomic operations)...\n");
        run_fixed_atomic_test();
    } else if use_fixed {
        println!("Running FIXED version (atomic with mutex)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (non-atomic read-modify-write)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let api = Arc::new(buggy::ClientAPI::new());
    let mut handles = vec![];

    // Simulate 10 concurrent transactions adding to the same account
    let account = "alice";
    let num_threads = 10;
    let amount_per_thread = 100;

    for i in 0..num_threads {
        let api = Arc::clone(&api);
        let handle = thread::spawn(move || {
            println!("[BUGGY] Thread {} adding order...", i);
            api.add_pending_order(account, amount_per_thread);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    thread::sleep(Duration::from_millis(100));

    let expected = num_threads * amount_per_thread;
    let actual = api.get_pending(account);

    println!("\n=== Results ===");
    println!("Expected total: {}", expected);
    println!("Actual total: {}", actual);

    api.check_lost_updates(account, expected);
    let lost = api.get_lost_updates();

    if lost > 0 {
        println!("\n[BUG DEMONSTRATED]");
        println!("Lost {} units due to non-atomic read-modify-write!", lost);
        println!("This is a classic 'lost update' atomicity violation.");
        println!("In Sui, this could enable double-spending attacks.");
    } else {
        println!("\n[NOTE]");
        println!("No lost updates this run (timing-dependent race).");
        println!("Try running multiple times to see the bug.");
    }

    println!("\nRun with --fixed to see atomic mutex version.");
    println!("Run with --atomic to see atomic operations version.");
}

fn run_fixed_test() {
    let api = Arc::new(fixed::ClientAPI::new());
    let mut handles = vec![];

    let account = "alice";
    let num_threads = 10;
    let amount_per_thread = 100;

    for i in 0..num_threads {
        let api = Arc::clone(&api);
        let handle = thread::spawn(move || {
            println!("[FIXED] Thread {} adding order...", i);
            api.add_pending_order(account, amount_per_thread);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let expected = num_threads * amount_per_thread;
    let actual = api.get_pending(account);

    println!("\n=== Results ===");
    println!("Expected total: {}", expected);
    println!("Actual total: {}", actual);

    if actual == expected {
        println!("\n[FIXED]");
        println!("All updates preserved! Atomic read-modify-write with Mutex.");
        println!("The entire sequence is protected by a single lock.");
    } else {
        println!("\n[ERROR]");
        println!("Unexpected result (should not happen with fix).");
    }
}

fn run_fixed_atomic_test() {
    let api = Arc::new(fixed_atomic::ClientAPI::new());
    let mut handles = vec![];

    let account = "alice";
    let num_threads = 10;
    let amount_per_thread = 100;

    for i in 0..num_threads {
        let api = Arc::clone(&api);
        let handle = thread::spawn(move || {
            println!("[FIXED-ATOMIC] Thread {} adding order...", i);
            api.add_pending_order(account, amount_per_thread);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let expected = num_threads * amount_per_thread;
    let actual = api.get_pending(account);

    println!("\n=== Results ===");
    println!("Expected total: {}", expected);
    println!("Actual total: {}", actual);

    if actual == expected {
        println!("\n[FIXED-ATOMIC]");
        println!("All updates preserved! Using AtomicU64::fetch_add.");
        println!("Lock-free atomic operations ensure no updates are lost.");
    } else {
        println!("\n[ERROR]");
        println!("Unexpected result (should not happen with fix).");
    }
}
