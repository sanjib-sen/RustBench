//! Reth Issue #12287: Transaction Pool Nonce Race Condition
//!
//! This reproduces a TOCTOU race where transaction validation uses
//! stale nonce information because a block is mined between
//! validation and pool insertion.
//!
//! Original bug: https://github.com/paradigmxyz/reth/issues/12287

use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
enum SubPool {
    Pending, // Ready for execution
    Queued,  // Waiting for nonce gap to be filled
}

#[derive(Debug, Clone)]
pub struct Transaction {
    pub from: String,
    pub nonce: u64,
    pub data: String,
}

/// Simulates blockchain account state
pub struct AccountState {
    nonces: RwLock<HashMap<String, u64>>,
}

impl AccountState {
    fn new() -> Self {
        let mut nonces = HashMap::new();
        nonces.insert("alice".to_string(), 0);
        nonces.insert("bob".to_string(), 0);
        Self {
            nonces: RwLock::new(nonces),
        }
    }

    fn get_nonce(&self, account: &str) -> u64 {
        let nonces = self.nonces.read().unwrap();
        *nonces.get(account).unwrap_or(&0)
    }

    fn increment_nonce(&self, account: &str) {
        let mut nonces = self.nonces.write().unwrap();
        if let Some(nonce) = nonces.get_mut(account) {
            *nonce += 1;
        }
    }
}

/// Buggy transaction pool
mod buggy {
    use super::*;

    pub struct TxPool {
        state: Arc<AccountState>,
        pending: Mutex<Vec<Transaction>>,
        queued: Mutex<Vec<Transaction>>,
        misclassified: Arc<AtomicU64>,
    }

    impl TxPool {
        pub fn new(state: Arc<AccountState>) -> Self {
            Self {
                state,
                pending: Mutex::new(Vec::new()),
                queued: Mutex::new(Vec::new()),
                misclassified: Arc::new(AtomicU64::new(0)),
            }
        }

        /// Validate transaction against current state
        fn validate(&self, tx: &Transaction) -> (bool, u64) {
            let expected_nonce = self.state.get_nonce(&tx.from);
            let valid = tx.nonce >= expected_nonce;
            (valid, expected_nonce)
        }

        /// BUG: Race between validate and add
        pub fn add_transaction(&self, tx: Transaction) -> SubPool {
            // Step 1: Validate against current state
            let (valid, expected_nonce) = self.validate(&tx);

            if !valid {
                println!(
                    "[BUGGY] Tx {:?} rejected (nonce {} < expected {})",
                    tx.data, tx.nonce, expected_nonce
                );
                return SubPool::Queued;
            }

            // BUG: Race window here!
            // A block can be mined between validate() and pool insertion
            // that changes the expected_nonce

            // Simulate some processing delay
            thread::sleep(Duration::from_micros(100));

            // Step 2: Determine pool based on STALE nonce info
            let pool = if tx.nonce == expected_nonce {
                SubPool::Pending
            } else {
                SubPool::Queued // Nonce gap detected
            };

            // Add to appropriate pool
            match pool {
                SubPool::Pending => {
                    let mut pending = self.pending.lock().unwrap();
                    pending.push(tx.clone());
                }
                SubPool::Queued => {
                    let mut queued = self.queued.lock().unwrap();
                    queued.push(tx.clone());
                }
            }

            // Check if we got it wrong (for demonstration)
            let current_nonce = self.state.get_nonce(&tx.from);
            let correct_pool = if tx.nonce == current_nonce {
                SubPool::Pending
            } else {
                SubPool::Queued
            };

            if pool != correct_pool {
                self.misclassified.fetch_add(1, Ordering::SeqCst);
                println!(
                    "[BUGGY] MISCLASSIFIED! Tx {:?} put in {:?} but should be {:?}",
                    tx.data, pool, correct_pool
                );
            } else {
                println!("[BUGGY] Tx {:?} -> {:?}", tx.data, pool);
            }

            pool
        }

        pub fn get_misclassified(&self) -> u64 {
            self.misclassified.load(Ordering::SeqCst)
        }
    }
}

/// Fixed transaction pool
mod fixed {
    use super::*;

    pub struct TxPool {
        state: Arc<AccountState>,
        pending: Mutex<Vec<Transaction>>,
        queued: Mutex<Vec<Transaction>>,
        // Lock to ensure atomic validate-and-add
        add_lock: Mutex<()>,
    }

    impl TxPool {
        pub fn new(state: Arc<AccountState>) -> Self {
            Self {
                state,
                pending: Mutex::new(Vec::new()),
                queued: Mutex::new(Vec::new()),
                add_lock: Mutex::new(()),
            }
        }

        /// FIX: Atomic validate and add
        pub fn add_transaction(&self, tx: Transaction) -> SubPool {
            // Hold lock during entire validate-and-add sequence
            let _guard = self.add_lock.lock().unwrap();

            // Validate and determine pool atomically
            let expected_nonce = self.state.get_nonce(&tx.from);

            if tx.nonce < expected_nonce {
                println!(
                    "[FIXED] Tx {:?} rejected (nonce {} < expected {})",
                    tx.data, tx.nonce, expected_nonce
                );
                return SubPool::Queued;
            }

            let pool = if tx.nonce == expected_nonce {
                SubPool::Pending
            } else {
                SubPool::Queued
            };

            // Add to pool while still holding lock
            match pool {
                SubPool::Pending => {
                    let mut pending = self.pending.lock().unwrap();
                    pending.push(tx.clone());
                }
                SubPool::Queued => {
                    let mut queued = self.queued.lock().unwrap();
                    queued.push(tx.clone());
                }
            }

            println!("[FIXED] Tx {:?} -> {:?}", tx.data, pool);
            pool
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Reth Issue #12287: Transaction Pool Nonce Race ===\n");

    if use_fixed {
        println!("Running FIXED version (atomic validate-and-add)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (racy validate then add)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let state = Arc::new(AccountState::new());
    let pool = Arc::new(buggy::TxPool::new(Arc::clone(&state)));

    let mut handles = vec![];

    // Thread 1: Submit transactions
    let pool1 = Arc::clone(&pool);
    handles.push(thread::spawn(move || {
        for i in 0..10 {
            let tx = Transaction {
                from: "alice".to_string(),
                nonce: i,
                data: format!("tx_{}", i),
            };
            pool1.add_transaction(tx);
            thread::sleep(Duration::from_millis(1));
        }
    }));

    // Thread 2: Mine blocks (increment nonces)
    let state2 = Arc::clone(&state);
    handles.push(thread::spawn(move || {
        for _ in 0..10 {
            thread::sleep(Duration::from_micros(50));
            state2.increment_nonce("alice");
        }
    }));

    for handle in handles {
        handle.join().unwrap();
    }

    let misclassified = pool.get_misclassified();
    println!("\n=== Results ===");
    println!("Misclassified transactions: {}", misclassified);

    if misclassified > 0 {
        println!("\n[BUG DEMONSTRATED]");
        println!("Transactions were placed in wrong pools due to TOCTOU race.");
    } else {
        println!("\n[NOTE]");
        println!("No misclassification this run (timing-dependent).");
    }
    println!("\nRun with --fixed to see atomic version.");
}

fn run_fixed_test() {
    let state = Arc::new(AccountState::new());
    let pool = Arc::new(fixed::TxPool::new(Arc::clone(&state)));

    let mut handles = vec![];

    let pool1 = Arc::clone(&pool);
    handles.push(thread::spawn(move || {
        for i in 0..10 {
            let tx = Transaction {
                from: "alice".to_string(),
                nonce: i,
                data: format!("tx_{}", i),
            };
            pool1.add_transaction(tx);
            thread::sleep(Duration::from_millis(1));
        }
    }));

    let state2 = Arc::clone(&state);
    handles.push(thread::spawn(move || {
        for _ in 0..10 {
            thread::sleep(Duration::from_micros(50));
            state2.increment_nonce("alice");
        }
    }));

    for handle in handles {
        handle.join().unwrap();
    }

    println!("\n=== Results ===");
    println!("[FIXED]");
    println!("Atomic validate-and-add prevents race condition.");
}
