//! Sui Issue #960: Object Lock Deadlock
//!
//! This reproduces a deadlock where transaction objects are locked but
//! never unlocked due to errors during transaction execution, causing
//! subsequent transactions to deadlock waiting for the same objects.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/960

use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

#[derive(Debug, Clone, PartialEq)]
pub struct TransactionId(String);

#[derive(Debug)]
pub enum TransactionError {
    NetworkError(String),
    ExecutionError(String),
    ObjectLocked(ObjectId),
}

/// Tracks which transaction has locked which objects
pub struct ObjectLockManager {
    locked_objects: Mutex<HashMap<ObjectId, TransactionId>>,
}

impl ObjectLockManager {
    fn new() -> Self {
        Self {
            locked_objects: Mutex::new(HashMap::new()),
        }
    }

    fn try_lock_objects(
        &self,
        objects: &[ObjectId],
        tx_id: &TransactionId,
    ) -> Result<(), TransactionError> {
        let mut locked = self.locked_objects.lock().unwrap();

        // Check if any object is already locked by a different transaction
        for obj in objects {
            if let Some(existing_tx) = locked.get(obj) {
                if existing_tx != tx_id {
                    return Err(TransactionError::ObjectLocked(obj.clone()));
                }
            }
        }

        // Lock all objects
        for obj in objects {
            locked.insert(obj.clone(), tx_id.clone());
            println!(
                "  [LOCK] Object {:?} locked by transaction {:?}",
                obj.0, tx_id.0
            );
        }

        Ok(())
    }

    fn unlock_objects(&self, objects: &[ObjectId]) {
        let mut locked = self.locked_objects.lock().unwrap();
        for obj in objects {
            locked.remove(obj);
            println!("  [UNLOCK] Object {:?} unlocked", obj.0);
        }
    }

    fn is_locked(&self, obj: &ObjectId) -> bool {
        let locked = self.locked_objects.lock().unwrap();
        locked.contains_key(obj)
    }
}

/// Simulates network/execution failures
fn simulate_transaction_execution(tx_id: &TransactionId) -> Result<(), TransactionError> {
    // Simulate work
    thread::sleep(Duration::from_millis(50));

    // Simulate failure for specific transactions
    if tx_id.0.contains("fail") {
        return Err(TransactionError::NetworkError(
            "Broken pipe".to_string(),
        ));
    }

    Ok(())
}

/// Buggy gateway state - missing unlock on error path
mod buggy {
    use super::*;

    pub struct GatewayState {
        lock_manager: Arc<ObjectLockManager>,
    }

    impl GatewayState {
        pub fn new(lock_manager: Arc<ObjectLockManager>) -> Self {
            Self { lock_manager }
        }

        /// BUG: Unlock not called on error path
        pub fn execute_transaction(
            &self,
            tx_id: TransactionId,
            objects: Vec<ObjectId>,
        ) -> Result<(), TransactionError> {
            println!("[BUGGY] Executing transaction {:?}", tx_id.0);

            // Lock objects for this transaction
            self.lock_manager.try_lock_objects(&objects, &tx_id)?;

            // Execute transaction
            let result = simulate_transaction_execution(&tx_id);

            // BUG: Only unlock on success!
            if result.is_ok() {
                self.lock_manager.unlock_objects(&objects);
                println!("[BUGGY] Transaction {:?} succeeded", tx_id.0);
            } else {
                // BUG: Missing unlock on error!
                println!(
                    "[BUGGY] Transaction {:?} failed: {:?}",
                    tx_id.0,
                    result.as_ref().unwrap_err()
                );
                println!(
                    "[BUGGY] WARNING: Objects remain locked! (unlock not called)"
                );
            }

            result
        }
    }
}

/// Fixed gateway state - ensures unlock even on error
mod fixed {
    use super::*;

    pub struct GatewayState {
        lock_manager: Arc<ObjectLockManager>,
    }

    impl GatewayState {
        pub fn new(lock_manager: Arc<ObjectLockManager>) -> Self {
            Self { lock_manager }
        }

        /// FIX: Always unlock, even on error
        pub fn execute_transaction(
            &self,
            tx_id: TransactionId,
            objects: Vec<ObjectId>,
        ) -> Result<(), TransactionError> {
            println!("[FIXED] Executing transaction {:?}", tx_id.0);

            // Lock objects for this transaction
            self.lock_manager.try_lock_objects(&objects, &tx_id)?;

            // Execute transaction
            let result = simulate_transaction_execution(&tx_id);

            // FIX: Always unlock, regardless of result
            self.lock_manager.unlock_objects(&objects);

            match &result {
                Ok(_) => println!("[FIXED] Transaction {:?} succeeded", tx_id.0),
                Err(e) => {
                    println!("[FIXED] Transaction {:?} failed: {:?}", tx_id.0, e);
                    println!("[FIXED] Objects properly unlocked despite error");
                }
            }

            result
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #960: Object Lock Deadlock ===\n");

    if use_fixed {
        println!("Running FIXED version (unlock on all paths)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (missing unlock on error)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let lock_manager = Arc::new(ObjectLockManager::new());
    let gateway = Arc::new(buggy::GatewayState::new(Arc::clone(&lock_manager)));

    // Transaction 1: Will fail, leaving object locked
    let tx1 = TransactionId("tx_1_fail".to_string());
    let obj_a = ObjectId("object_A".to_string());

    println!("--- Transaction 1 (will fail) ---");
    let _ = gateway.execute_transaction(tx1, vec![obj_a.clone()]);

    thread::sleep(Duration::from_millis(100));

    // Check if object is still locked
    if lock_manager.is_locked(&obj_a) {
        println!("\n[BUG DETECTED] Object {:?} is still locked!", obj_a.0);
    }

    // Transaction 2: Tries to use the same object, will deadlock
    println!("\n--- Transaction 2 (will deadlock) ---");
    let tx2 = TransactionId("tx_2".to_string());

    println!("[BUGGY] Attempting transaction {:?}", tx2.0);
    match gateway.execute_transaction(tx2, vec![obj_a.clone()]) {
        Err(TransactionError::ObjectLocked(obj)) => {
            println!(
                "[BUGGY] DEADLOCK! Transaction blocked - object {:?} still locked from previous failed transaction",
                obj.0
            );
        }
        _ => println!("[BUGGY] Transaction succeeded unexpectedly"),
    }

    println!("\n=== Results ===");
    println!("[BUG DEMONSTRATED]");
    println!("First transaction failed and left object locked.");
    println!("Second transaction deadlocked trying to acquire the same lock.");
    println!("In production, this causes HTTP 424 errors: 'Client state has a different pending transaction'");
    println!("\nRun with --fixed to see proper unlock handling.");
}

fn run_fixed_test() {
    let lock_manager = Arc::new(ObjectLockManager::new());
    let gateway = Arc::new(fixed::GatewayState::new(Arc::clone(&lock_manager)));

    // Transaction 1: Will fail, but unlock properly
    let tx1 = TransactionId("tx_1_fail".to_string());
    let obj_a = ObjectId("object_A".to_string());

    println!("--- Transaction 1 (will fail) ---");
    let _ = gateway.execute_transaction(tx1, vec![obj_a.clone()]);

    thread::sleep(Duration::from_millis(100));

    // Check if object is unlocked
    if !lock_manager.is_locked(&obj_a) {
        println!("\n[FIXED] Object {:?} properly unlocked", obj_a.0);
    }

    // Transaction 2: Should succeed now
    println!("\n--- Transaction 2 (should succeed) ---");
    let tx2 = TransactionId("tx_2".to_string());

    match gateway.execute_transaction(tx2, vec![obj_a.clone()]) {
        Ok(_) => {
            println!("[FIXED] Transaction completed successfully!");
            println!("[FIXED] No deadlock - object was properly released");
        }
        Err(TransactionError::ObjectLocked(obj)) => {
            println!("[FIXED] ERROR: Object {:?} still locked (should not happen)", obj.0);
        }
        Err(e) => {
            println!("[FIXED] Transaction failed with error: {:?}", e);
        }
    }

    println!("\n=== Results ===");
    println!("[FIXED]");
    println!("First transaction failed but properly unlocked objects.");
    println!("Second transaction succeeded - no deadlock.");
    println!("Unlock is guaranteed on all code paths (success and error).");
}
