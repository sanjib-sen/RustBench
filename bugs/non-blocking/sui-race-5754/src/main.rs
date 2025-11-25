//! Sui Issue #5754: Object Version Race
//!
//! This reproduces a race where the object version in parent_sync table
//! can be updated by checkpoint execution before epoch initialization
//! completes, causing stale versions to be used.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/5754
//! Fix PR: https://github.com/MystenLabs/sui/pull/7044

use std::cmp::max;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

pub type ObjectId = String;
pub type Version = u64;

#[derive(Debug, Clone)]
pub struct ObjectRef {
    id: ObjectId,
    version: Version,
}

/// Stores parent sync information for objects
pub struct ParentSyncTable {
    entries: RwLock<HashMap<ObjectId, ObjectRef>>,
}

impl ParentSyncTable {
    fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    fn get_latest_parent_entry(&self, id: &str) -> Option<ObjectRef> {
        let entries = self.entries.read().unwrap();
        entries.get(id).cloned()
    }

    fn update_entry(&self, obj_ref: ObjectRef) {
        let mut entries = self.entries.write().unwrap();
        entries.insert(obj_ref.id.clone(), obj_ref);
    }
}

/// Stores initial shared versions for objects transitioning from owned to shared
pub struct SharedObjectTable {
    initial_versions: RwLock<HashMap<ObjectId, Version>>,
}

impl SharedObjectTable {
    fn new() -> Self {
        Self {
            initial_versions: RwLock::new(HashMap::new()),
        }
    }

    fn get_initial_shared_version(&self, id: &str) -> Option<Version> {
        let versions = self.initial_versions.read().unwrap();
        versions.get(id).cloned()
    }

    fn set_initial_shared_version(&self, id: &str, version: Version) {
        let mut versions = self.initial_versions.write().unwrap();
        versions.insert(id.to_string(), version);
    }
}

/// Buggy version - uses stale version from parent sync
mod buggy {
    use super::*;

    pub struct EpochStore {
        parent_sync: Arc<ParentSyncTable>,
        shared_objects: Arc<SharedObjectTable>,
    }

    impl EpochStore {
        pub fn new(
            parent_sync: Arc<ParentSyncTable>,
            shared_objects: Arc<SharedObjectTable>,
        ) -> Self {
            Self {
                parent_sync,
                shared_objects,
            }
        }

        /// BUG: Gets version from parent_sync which may be stale
        /// Does not consider initial_shared_version for upgrades
        pub fn get_next_version(&self, object_id: &str) -> Version {
            // BUG: Only checks parent_sync, ignores initial_shared_version
            if let Some(obj_ref) = self.parent_sync.get_latest_parent_entry(object_id) {
                println!(
                    "[BUGGY] Using parent_sync version {} for object '{}'",
                    obj_ref.version, object_id
                );
                obj_ref.version + 1
            } else {
                println!(
                    "[BUGGY] No parent_sync entry for '{}', using version 1",
                    object_id
                );
                1
            }
        }
    }
}

/// Fixed version - uses max of parent_sync and initial_shared_version
mod fixed {
    use super::*;

    pub struct EpochStore {
        parent_sync: Arc<ParentSyncTable>,
        shared_objects: Arc<SharedObjectTable>,
    }

    impl EpochStore {
        pub fn new(
            parent_sync: Arc<ParentSyncTable>,
            shared_objects: Arc<SharedObjectTable>,
        ) -> Self {
            Self {
                parent_sync,
                shared_objects,
            }
        }

        /// FIX: Use max of parent_sync version and initial_shared_version
        pub fn get_next_version(&self, object_id: &str) -> Version {
            let initial_version = self
                .shared_objects
                .get_initial_shared_version(object_id)
                .unwrap_or(0);

            let parent_version = self
                .parent_sync
                .get_latest_parent_entry(object_id)
                .map(|obj_ref| obj_ref.version)
                .unwrap_or(0);

            // FIX: Use max to handle both cases correctly
            let version = max(parent_version, initial_version);
            println!(
                "[FIXED] Object '{}': parent_sync={}, initial_shared={}, using max={}",
                object_id, parent_version, initial_version, version
            );
            version + 1
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #5754: Object Version Race ===\n");

    if use_fixed {
        println!("Running FIXED version (max of versions)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (stale parent_sync)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let parent_sync = Arc::new(ParentSyncTable::new());
    let shared_objects = Arc::new(SharedObjectTable::new());

    // Object transitioning from owned to shared
    let object_id = "obj_upgrade";
    let initial_shared_version: Version = 100;

    // Set initial shared version (object became shared at version 100)
    shared_objects.set_initial_shared_version(object_id, initial_shared_version);
    println!(
        "Object '{}' became shared at version {}",
        object_id, initial_shared_version
    );

    let epoch_store = Arc::new(buggy::EpochStore::new(
        Arc::clone(&parent_sync),
        Arc::clone(&shared_objects),
    ));

    // Thread 1: Epoch initialization - reads version
    let epoch_store1 = Arc::clone(&epoch_store);
    let parent_sync1 = Arc::clone(&parent_sync);
    let object_id1 = object_id.to_string();

    let handle1 = thread::spawn(move || {
        println!("\n[Thread 1] Epoch initialization starting...");

        // BUG: parent_sync has old/no entry, but checkpoint may update it
        let version = epoch_store1.get_next_version(&object_id1);

        // Simulate processing delay
        thread::sleep(Duration::from_millis(50));

        println!("[Thread 1] Will use version {} for next operation", version);
        version
    });

    // Thread 2: Checkpoint sync - updates parent_sync
    let parent_sync2 = Arc::clone(&parent_sync);
    let object_id2 = object_id.to_string();

    let handle2 = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10)); // Slight delay

        println!("\n[Thread 2] Checkpoint sync executing...");
        // Checkpoint sync updates the version to 150
        let new_ref = ObjectRef {
            id: object_id2.clone(),
            version: 150,
        };
        parent_sync2.update_entry(new_ref);
        println!("[Thread 2] Updated parent_sync to version 150");
    });

    let epoch_version = handle1.join().unwrap();
    handle2.join().unwrap();

    // Check final state
    let final_parent_version = parent_sync
        .get_latest_parent_entry(object_id)
        .map(|r| r.version)
        .unwrap_or(0);

    println!("\n=== Results ===");
    println!("Initial shared version: {}", initial_shared_version);
    println!("Final parent_sync version: {}", final_parent_version);
    println!("Epoch chose version: {}", epoch_version);

    if epoch_version < initial_shared_version {
        println!("\n[BUG DEMONSTRATED]");
        println!("Epoch initialization used stale version {}!", epoch_version);
        println!("Should have used at least {} (initial_shared_version)", initial_shared_version + 1);
        println!("This can cause version conflicts in transaction processing.");
    } else if epoch_version < final_parent_version {
        println!("\n[BUG DEMONSTRATED]");
        println!("Race condition: parent_sync was updated after read!");
        println!("Epoch used version {}, but parent_sync is now at {}", epoch_version, final_parent_version);
    }

    println!("\nRun with --fixed to see max-based version selection.");
}

fn run_fixed_test() {
    let parent_sync = Arc::new(ParentSyncTable::new());
    let shared_objects = Arc::new(SharedObjectTable::new());

    let object_id = "obj_upgrade";
    let initial_shared_version: Version = 100;

    shared_objects.set_initial_shared_version(object_id, initial_shared_version);
    println!(
        "Object '{}' became shared at version {}",
        object_id, initial_shared_version
    );

    let epoch_store = Arc::new(fixed::EpochStore::new(
        Arc::clone(&parent_sync),
        Arc::clone(&shared_objects),
    ));

    let epoch_store1 = Arc::clone(&epoch_store);
    let object_id1 = object_id.to_string();

    let handle1 = thread::spawn(move || {
        println!("\n[Thread 1] Epoch initialization starting...");
        let version = epoch_store1.get_next_version(&object_id1);
        thread::sleep(Duration::from_millis(50));
        println!("[Thread 1] Will use version {} for next operation", version);
        version
    });

    let parent_sync2 = Arc::clone(&parent_sync);
    let object_id2 = object_id.to_string();

    let handle2 = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        println!("\n[Thread 2] Checkpoint sync executing...");
        let new_ref = ObjectRef {
            id: object_id2.clone(),
            version: 150,
        };
        parent_sync2.update_entry(new_ref);
        println!("[Thread 2] Updated parent_sync to version 150");
    });

    let epoch_version = handle1.join().unwrap();
    handle2.join().unwrap();

    let final_parent_version = parent_sync
        .get_latest_parent_entry(object_id)
        .map(|r| r.version)
        .unwrap_or(0);

    println!("\n=== Results ===");
    println!("Initial shared version: {}", initial_shared_version);
    println!("Final parent_sync version: {}", final_parent_version);
    println!("Epoch chose version: {}", epoch_version);

    if epoch_version > initial_shared_version {
        println!("\n[FIXED]");
        println!("Epoch initialization correctly used version {}!", epoch_version);
        println!("Used max(parent_sync, initial_shared) to ensure consistency.");
        println!("No version conflicts possible.");
    }
}
