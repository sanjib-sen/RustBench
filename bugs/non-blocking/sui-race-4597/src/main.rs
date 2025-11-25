//! Sui Issue #4597: Gas Object Version Race
//!
//! This reproduces a race condition where a transaction uses the "latest"
//! version of a gas object instead of the version specified in the request.
//! This causes inconsistency when concurrent transactions update the gas object.
//!
//! Original Issue: https://github.com/MystenLabs/sui/issues/4597
//! Fix PR: https://github.com/MystenLabs/sui/pull/4588

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

pub type ObjectId = String;
pub type SequenceNumber = u64;
pub type TxDigest = String;

/// Represents a gas object with version tracking
#[derive(Clone, Debug)]
pub struct GasObject {
    id: ObjectId,
    version: SequenceNumber,
    balance: u64,
}

/// Transaction request that specifies gas object and version
#[derive(Clone, Debug)]
pub struct TransactionRequest {
    digest: TxDigest,
    gas_object_id: ObjectId,
    gas_version: SequenceNumber, // Version expected by the request
    gas_required: u64,
}

/// Object store that tracks the latest version of each object
pub struct ObjectStore {
    objects: RwLock<HashMap<ObjectId, GasObject>>,
}

impl ObjectStore {
    fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
        }
    }

    fn insert(&self, obj: GasObject) {
        self.objects.write().unwrap().insert(obj.id.clone(), obj);
    }

    fn get_latest(&self, id: &str) -> Option<GasObject> {
        self.objects.read().unwrap().get(id).cloned()
    }

    fn get_at_version(&self, id: &str, version: SequenceNumber) -> Option<GasObject> {
        let obj = self.objects.read().unwrap().get(id).cloned();
        // Simulate version check
        obj.filter(|o| o.version == version)
    }

    fn update(&self, id: &str, new_balance: u64) -> Option<GasObject> {
        let mut objects = self.objects.write().unwrap();
        if let Some(obj) = objects.get_mut(id) {
            obj.version += 1;
            obj.balance = new_balance;
            return Some(obj.clone());
        }
        None
    }
}

/// Transaction results
#[derive(Debug, Clone)]
pub struct TransactionResult {
    digest: TxDigest,
    success: bool,
    gas_used: u64,
    gas_version_used: SequenceNumber,
    error: Option<String>,
}

/// Buggy version - uses latest gas version instead of request version
mod buggy {
    use super::*;

    pub struct TransactionProcessor {
        store: Arc<ObjectStore>,
        results: Mutex<Vec<TransactionResult>>,
    }

    impl TransactionProcessor {
        pub fn new(store: Arc<ObjectStore>) -> Self {
            Self {
                store,
                results: Mutex::new(Vec::new()),
            }
        }

        /// BUG: Uses latest gas object version, not the request version
        pub fn execute(&self, request: &TransactionRequest) {
            println!("[BUGGY] Processing tx {} (requested gas version: {})",
                     request.digest, request.gas_version);

            // BUG: Get latest version instead of request version
            let gas_obj = match self.store.get_latest(&request.gas_object_id) {
                Some(obj) => obj,
                None => {
                    self.record_result(TransactionResult {
                        digest: request.digest.clone(),
                        success: false,
                        gas_used: 0,
                        gas_version_used: 0,
                        error: Some("Gas object not found".to_string()),
                    });
                    return;
                }
            };

            println!("[BUGGY] Tx {} got gas version {} (requested {})",
                     request.digest, gas_obj.version, request.gas_version);

            // BUG: Race condition!
            // If another tx updated the gas object, we're using wrong version
            if gas_obj.version != request.gas_version {
                println!("[BUGGY] VERSION MISMATCH! Tx {} expected version {}, got {}",
                         request.digest, request.gas_version, gas_obj.version);
                // In buggy version, we proceed anyway with wrong version
            }

            // Simulate some processing time (widens race window)
            thread::sleep(Duration::from_millis(20));

            // Check balance
            if gas_obj.balance < request.gas_required {
                self.record_result(TransactionResult {
                    digest: request.digest.clone(),
                    success: false,
                    gas_used: 0,
                    gas_version_used: gas_obj.version,
                    error: Some(format!("Insufficient gas: {} < {}",
                                       gas_obj.balance, request.gas_required)),
                });
                return;
            }

            // Deduct gas (updates version)
            let new_balance = gas_obj.balance - request.gas_required;
            let updated = self.store.update(&request.gas_object_id, new_balance);

            self.record_result(TransactionResult {
                digest: request.digest.clone(),
                success: true,
                gas_used: request.gas_required,
                gas_version_used: gas_obj.version,
                error: None,
            });

            println!("[BUGGY] Tx {} completed, gas object now at version {}",
                     request.digest, updated.map(|o| o.version).unwrap_or(0));
        }

        fn record_result(&self, result: TransactionResult) {
            self.results.lock().unwrap().push(result);
        }

        pub fn get_results(&self) -> Vec<TransactionResult> {
            self.results.lock().unwrap().clone()
        }
    }
}

/// Fixed version - uses request version and validates
mod fixed {
    use super::*;

    pub struct TransactionProcessor {
        store: Arc<ObjectStore>,
        results: Mutex<Vec<TransactionResult>>,
    }

    impl TransactionProcessor {
        pub fn new(store: Arc<ObjectStore>) -> Self {
            Self {
                store,
                results: Mutex::new(Vec::new()),
            }
        }

        /// FIX: Use request version and validate it matches
        pub fn execute(&self, request: &TransactionRequest) {
            println!("[FIXED] Processing tx {} (requested gas version: {})",
                     request.digest, request.gas_version);

            // FIX: Get gas object at the specific requested version
            let gas_obj = match self.store.get_at_version(
                &request.gas_object_id,
                request.gas_version
            ) {
                Some(obj) => obj,
                None => {
                    // FIX: Version mismatch is an error, not silently ignored
                    let latest = self.store.get_latest(&request.gas_object_id);
                    let error_msg = match latest {
                        Some(obj) => format!(
                            "Version mismatch: requested {}, current {}",
                            request.gas_version, obj.version
                        ),
                        None => "Gas object not found".to_string(),
                    };

                    println!("[FIXED] Tx {} failed: {}", request.digest, error_msg);

                    self.record_result(TransactionResult {
                        digest: request.digest.clone(),
                        success: false,
                        gas_used: 0,
                        gas_version_used: request.gas_version,
                        error: Some(error_msg),
                    });
                    return;
                }
            };

            println!("[FIXED] Tx {} got gas version {} (matches request)",
                     request.digest, gas_obj.version);

            thread::sleep(Duration::from_millis(20));

            if gas_obj.balance < request.gas_required {
                self.record_result(TransactionResult {
                    digest: request.digest.clone(),
                    success: false,
                    gas_used: 0,
                    gas_version_used: gas_obj.version,
                    error: Some(format!("Insufficient gas: {} < {}",
                                       gas_obj.balance, request.gas_required)),
                });
                return;
            }

            let new_balance = gas_obj.balance - request.gas_required;
            let updated = self.store.update(&request.gas_object_id, new_balance);

            self.record_result(TransactionResult {
                digest: request.digest.clone(),
                success: true,
                gas_used: request.gas_required,
                gas_version_used: gas_obj.version,
                error: None,
            });

            println!("[FIXED] Tx {} completed, gas object now at version {}",
                     request.digest, updated.map(|o| o.version).unwrap_or(0));
        }

        fn record_result(&self, result: TransactionResult) {
            self.results.lock().unwrap().push(result);
        }

        pub fn get_results(&self) -> Vec<TransactionResult> {
            self.results.lock().unwrap().clone()
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #4597: Gas Object Version Race ===\n");

    if use_fixed {
        println!("Running FIXED version (use request version, validate match)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (use latest version, ignore mismatch)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let store = Arc::new(ObjectStore::new());

    // Create gas object with version 1
    store.insert(GasObject {
        id: "gas_001".to_string(),
        version: 1,
        balance: 1000,
    });

    let processor = Arc::new(buggy::TransactionProcessor::new(Arc::clone(&store)));

    println!("Scenario: Two transactions created when gas was at version 1");
    println!("tx1 executes first, updates gas to version 2");
    println!("tx2 executes after but still expects version 1\n");

    // Transaction 1: created when gas was at version 1
    let tx1 = TransactionRequest {
        digest: "tx_001".to_string(),
        gas_object_id: "gas_001".to_string(),
        gas_version: 1, // Expects version 1
        gas_required: 400,
    };

    // Transaction 2: also created when gas was at version 1
    // But by the time it executes, gas is at version 2
    let tx2 = TransactionRequest {
        digest: "tx_002".to_string(),
        gas_object_id: "gas_001".to_string(),
        gas_version: 1, // Expects version 1, but will see version 2
        gas_required: 300,
    };

    // Execute tx1 first, completely
    processor.execute(&tx1);

    // Now tx2 executes - gas is now at version 2
    // Bug: tx2 expects version 1 but gets version 2 and proceeds anyway
    processor.execute(&tx2);

    // Check results
    let results = processor.get_results();

    println!("\n=== Results ===");

    let mut version_mismatches = false;
    for result in &results {
        println!("{:?}", result);
        if let Some(ref _err) = result.error {
            if result.gas_version_used != 1 {
                version_mismatches = true;
            }
        }
    }

    // Check for the bug
    let successful: Vec<_> = results.iter().filter(|r| r.success).collect();

    if successful.len() == 2 || version_mismatches {
        println!("\n[BUG DEMONSTRATED]");
        println!("Problems observed:");
        println!("  - Transactions used latest gas version, not request version");
        println!("  - Version mismatch was silently ignored");
        println!("  - Lack of idempotency: same request can give different results");
        println!("  - Second tx may see updated version from first tx");
        println!("\nRun with --fixed to see version validation.");
    } else if successful.len() == 1 {
        println!("\nOne transaction succeeded (timing).");
        println!("Bug: The other tx may have used wrong version silently.");
        println!("Run with --fixed to see proper version validation.");
    }
}

fn run_fixed_test() {
    let store = Arc::new(ObjectStore::new());

    store.insert(GasObject {
        id: "gas_001".to_string(),
        version: 1,
        balance: 1000,
    });

    let processor = Arc::new(fixed::TransactionProcessor::new(Arc::clone(&store)));

    println!("Scenario: Two transactions created when gas was at version 1");
    println!("tx1 executes first, updates gas to version 2");
    println!("tx2 executes after but still expects version 1\n");

    let tx1 = TransactionRequest {
        digest: "tx_001".to_string(),
        gas_object_id: "gas_001".to_string(),
        gas_version: 1,
        gas_required: 400,
    };

    let tx2 = TransactionRequest {
        digest: "tx_002".to_string(),
        gas_object_id: "gas_001".to_string(),
        gas_version: 1, // Expects version 1, but gas is now at version 2
        gas_required: 300,
    };

    // Execute tx1 first, completely
    processor.execute(&tx1);

    // Now tx2 executes - gas is now at version 2
    // Fix: tx2 fails because version 1 != current version 2
    processor.execute(&tx2);

    let results = processor.get_results();

    println!("\n=== Results ===");
    for result in &results {
        println!("{:?}", result);
    }

    let successful: Vec<_> = results.iter().filter(|r| r.success).collect();
    let failed_version: Vec<_> = results.iter()
        .filter(|r| !r.success && r.error.as_ref().map(|e| e.contains("Version mismatch")).unwrap_or(false))
        .collect();

    if successful.len() == 1 && failed_version.len() == 1 {
        println!("\n[FIXED]");
        println!("One transaction succeeded, one failed with version mismatch!");
        println!("\nFix: Use request version, not latest version");
        println!("  - Validates gas version matches request");
        println!("  - Returns clear error on version mismatch");
        println!("  - Ensures idempotency: same request = same result");
        println!("  - Second tx must be retried with updated version");
    } else {
        println!("\nResults: {} succeeded, {} version errors",
                 successful.len(), failed_version.len());
    }
}
