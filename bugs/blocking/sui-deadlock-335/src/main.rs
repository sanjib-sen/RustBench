//! Sui Issue #335: Absence of Proper Locking
//!
//! This reproduces a blocking bug where simultaneous conflicting orders
//! can be submitted because there's no proper locking mechanism.
//! Multiple transactions can try to acquire the same object, leading to
//! conflicts that cause some transactions to block or fail.
//!
//! Original Issue: https://github.com/MystenLabs/sui/issues/335

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::Duration;

pub type ObjectId = String;
pub type TxDigest = String;

/// Represents an owned object that can only be used by one transaction at a time
#[derive(Clone, Debug)]
pub struct OwnedObject {
    id: ObjectId,
    owner: String,
    locked_by: Option<TxDigest>,
}

/// Represents a transaction order
#[derive(Clone, Debug)]
pub struct Order {
    digest: TxDigest,
    input_objects: Vec<ObjectId>,
}

/// Result of order processing
#[derive(Debug)]
pub enum OrderResult {
    Success,
    Conflict(String),
    Blocked,
}

/// Buggy version - no locking, allows conflicting orders
mod buggy {
    use super::*;

    pub struct Authority {
        objects: RwLock<HashMap<ObjectId, OwnedObject>>,
        pending_orders: Mutex<HashMap<ObjectId, Vec<TxDigest>>>,
    }

    impl Authority {
        pub fn new() -> Self {
            Self {
                objects: RwLock::new(HashMap::new()),
                pending_orders: Mutex::new(HashMap::new()),
            }
        }

        pub fn add_object(&self, obj: OwnedObject) {
            self.objects.write().unwrap().insert(obj.id.clone(), obj);
        }

        /// BUG: No locking mechanism - conflicting orders can be submitted simultaneously
        pub fn handle_order(&self, order: &Order) -> OrderResult {
            println!("[BUGGY] Processing order {} for objects {:?}",
                     order.digest, order.input_objects);

            // Check if objects exist and are owned
            {
                let objects = self.objects.read().unwrap();
                for obj_id in &order.input_objects {
                    if !objects.contains_key(obj_id) {
                        return OrderResult::Conflict(format!("Object {} not found", obj_id));
                    }
                }
            }

            // BUG: No lock acquired before processing!
            // Multiple orders for the same object can proceed simultaneously

            // Record pending order (but this isn't a lock!)
            {
                let mut pending = self.pending_orders.lock().unwrap();
                for obj_id in &order.input_objects {
                    pending.entry(obj_id.clone())
                        .or_insert_with(Vec::new)
                        .push(order.digest.clone());
                }
            }

            // Simulate processing time (opens race window)
            thread::sleep(Duration::from_millis(50));

            // Check for conflicts (too late - damage is done)
            {
                let pending = self.pending_orders.lock().unwrap();
                for obj_id in &order.input_objects {
                    if let Some(orders) = pending.get(obj_id) {
                        if orders.len() > 1 {
                            println!("[BUGGY] CONFLICT! Object {} has multiple orders: {:?}",
                                     obj_id, orders);
                            // BUG: We already started processing, now we have conflict
                        }
                    }
                }
            }

            // "Execute" the order
            {
                let mut objects = self.objects.write().unwrap();
                for obj_id in &order.input_objects {
                    if let Some(obj) = objects.get_mut(obj_id) {
                        // BUG: Both conflicting orders may execute!
                        obj.locked_by = Some(order.digest.clone());
                        println!("[BUGGY] Order {} acquired object {}", order.digest, obj_id);
                    }
                }
            }

            // Clean up pending
            {
                let mut pending = self.pending_orders.lock().unwrap();
                for obj_id in &order.input_objects {
                    if let Some(orders) = pending.get_mut(obj_id) {
                        orders.retain(|d| d != &order.digest);
                    }
                }
            }

            OrderResult::Success
        }

        pub fn get_object_holder(&self, obj_id: &str) -> Option<TxDigest> {
            self.objects.read().unwrap()
                .get(obj_id)
                .and_then(|obj| obj.locked_by.clone())
        }
    }
}

/// Fixed version - proper locking mechanism
mod fixed {
    use super::*;

    /// Lock entry for an object
    struct ObjectLock {
        locked_by: Option<TxDigest>,
        waiters: Vec<(TxDigest, Arc<(Mutex<bool>, Condvar)>)>,
    }

    pub struct Authority {
        objects: RwLock<HashMap<ObjectId, OwnedObject>>,
        object_locks: Mutex<HashMap<ObjectId, ObjectLock>>,
    }

    impl Authority {
        pub fn new() -> Self {
            Self {
                objects: RwLock::new(HashMap::new()),
                object_locks: Mutex::new(HashMap::new()),
            }
        }

        pub fn add_object(&self, obj: OwnedObject) {
            self.objects.write().unwrap().insert(obj.id.clone(), obj.clone());
            self.object_locks.lock().unwrap().insert(obj.id, ObjectLock {
                locked_by: None,
                waiters: Vec::new(),
            });
        }

        /// FIX: Acquire locks before processing
        pub fn handle_order(&self, order: &Order, wait_timeout: Duration) -> OrderResult {
            println!("[FIXED] Processing order {} for objects {:?}",
                     order.digest, order.input_objects);

            // Check if objects exist
            {
                let objects = self.objects.read().unwrap();
                for obj_id in &order.input_objects {
                    if !objects.contains_key(obj_id) {
                        return OrderResult::Conflict(format!("Object {} not found", obj_id));
                    }
                }
            }

            // FIX: Try to acquire locks on all input objects
            let mut acquired_locks = Vec::new();
            let start = std::time::Instant::now();

            for obj_id in &order.input_objects {
                loop {
                    let should_wait;
                    let waiter;

                    {
                        let mut locks = self.object_locks.lock().unwrap();
                        let lock_entry = locks.get_mut(obj_id).unwrap();

                        if lock_entry.locked_by.is_none() {
                            // FIX: Acquire the lock
                            lock_entry.locked_by = Some(order.digest.clone());
                            acquired_locks.push(obj_id.clone());
                            println!("[FIXED] Order {} acquired lock on {}",
                                     order.digest, obj_id);
                            break;
                        } else {
                            // Object is locked, need to wait
                            println!("[FIXED] Order {} waiting for {} (locked by {:?})",
                                     order.digest, obj_id, lock_entry.locked_by);

                            if start.elapsed() >= wait_timeout {
                                // Timeout - release acquired locks and return
                                self.release_locks(&order.digest, &acquired_locks);
                                return OrderResult::Blocked;
                            }

                            // Create waiter
                            let pair = Arc::new((Mutex::new(false), Condvar::new()));
                            lock_entry.waiters.push((order.digest.clone(), Arc::clone(&pair)));
                            waiter = pair;
                            should_wait = true;
                        }
                    }

                    if should_wait {
                        // Wait for lock to be released
                        let (lock, cvar) = &*waiter;
                        let guard = lock.lock().unwrap();
                        let remaining = wait_timeout.saturating_sub(start.elapsed());
                        let _ = cvar.wait_timeout(guard, remaining);
                    }
                }
            }

            // Simulate processing time
            thread::sleep(Duration::from_millis(50));

            // Execute the order
            {
                let mut objects = self.objects.write().unwrap();
                for obj_id in &order.input_objects {
                    if let Some(obj) = objects.get_mut(obj_id) {
                        obj.locked_by = Some(order.digest.clone());
                        println!("[FIXED] Order {} executed on {}", order.digest, obj_id);
                    }
                }
            }

            // Release locks
            self.release_locks(&order.digest, &acquired_locks);

            OrderResult::Success
        }

        fn release_locks(&self, tx_digest: &str, obj_ids: &[ObjectId]) {
            let mut locks = self.object_locks.lock().unwrap();
            for obj_id in obj_ids {
                if let Some(lock_entry) = locks.get_mut(obj_id) {
                    if lock_entry.locked_by.as_ref().map(|d| d == tx_digest).unwrap_or(false) {
                        lock_entry.locked_by = None;
                        println!("[FIXED] Order {} released lock on {}", tx_digest, obj_id);

                        // Wake up waiters
                        for (_, waiter) in lock_entry.waiters.drain(..) {
                            let (lock, cvar) = &*waiter;
                            *lock.lock().unwrap() = true;
                            cvar.notify_one();
                        }
                    }
                }
            }
        }

        pub fn get_object_holder(&self, obj_id: &str) -> Option<TxDigest> {
            self.objects.read().unwrap()
                .get(obj_id)
                .and_then(|obj| obj.locked_by.clone())
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #335: Absence of Proper Locking ===\n");

    if use_fixed {
        println!("Running FIXED version (proper locking)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (no locking, allows conflicts)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let authority = Arc::new(buggy::Authority::new());

    // Create a shared object
    authority.add_object(OwnedObject {
        id: "obj_001".to_string(),
        owner: "alice".to_string(),
        locked_by: None,
    });

    println!("Scenario: Two orders for the same object submitted simultaneously\n");

    let order1 = Order {
        digest: "order_001".to_string(),
        input_objects: vec!["obj_001".to_string()],
    };

    let order2 = Order {
        digest: "order_002".to_string(),
        input_objects: vec!["obj_001".to_string()],
    };

    let auth1 = Arc::clone(&authority);
    let auth2 = Arc::clone(&authority);

    // Submit both orders simultaneously
    let h1 = thread::spawn(move || {
        auth1.handle_order(&order1)
    });

    let h2 = thread::spawn(move || {
        auth2.handle_order(&order2)
    });

    let result1 = h1.join().unwrap();
    let result2 = h2.join().unwrap();

    println!("\n=== Results ===");
    println!("Order 1 result: {:?}", result1);
    println!("Order 2 result: {:?}", result2);

    let final_holder = authority.get_object_holder("obj_001");
    println!("Final object holder: {:?}", final_holder);

    // Check for bug
    let both_succeeded = matches!(result1, OrderResult::Success)
        && matches!(result2, OrderResult::Success);

    if both_succeeded {
        println!("\n[BUG DEMONSTRATED]");
        println!("Both conflicting orders succeeded!");
        println!("\nProblem:");
        println!("  - No locking mechanism to prevent conflicts");
        println!("  - Both orders executed on the same object");
        println!("  - Last writer wins (non-deterministic)");
        println!("\nRun with --fixed to see proper locking.");
    }
}

fn run_fixed_test() {
    let authority = Arc::new(fixed::Authority::new());

    authority.add_object(OwnedObject {
        id: "obj_001".to_string(),
        owner: "alice".to_string(),
        locked_by: None,
    });

    println!("Scenario: Two orders for the same object submitted simultaneously\n");

    let order1 = Order {
        digest: "order_001".to_string(),
        input_objects: vec!["obj_001".to_string()],
    };

    let order2 = Order {
        digest: "order_002".to_string(),
        input_objects: vec!["obj_001".to_string()],
    };

    let auth1 = Arc::clone(&authority);
    let auth2 = Arc::clone(&authority);

    let timeout = Duration::from_secs(2);

    let h1 = thread::spawn(move || {
        auth1.handle_order(&order1, timeout)
    });

    let h2 = thread::spawn(move || {
        auth2.handle_order(&order2, timeout)
    });

    let result1 = h1.join().unwrap();
    let result2 = h2.join().unwrap();

    println!("\n=== Results ===");
    println!("Order 1 result: {:?}", result1);
    println!("Order 2 result: {:?}", result2);

    let final_holder = authority.get_object_holder("obj_001");
    println!("Final object holder: {:?}", final_holder);

    let one_succeeded = matches!(result1, OrderResult::Success)
        ^ matches!(result2, OrderResult::Success);
    let both_succeeded = matches!(result1, OrderResult::Success)
        && matches!(result2, OrderResult::Success);

    if both_succeeded {
        println!("\n[FIXED - Sequential Execution]");
        println!("Both orders succeeded sequentially!");
        println!("\nFix: Proper locking ensures serial execution");
        println!("  - First order acquires lock");
        println!("  - Second order waits for lock");
        println!("  - Orders execute one at a time");
    } else if one_succeeded {
        println!("\n[FIXED]");
        println!("One order succeeded, one blocked/failed!");
        println!("\nFix: Proper locking prevents conflicts");
    }
}
