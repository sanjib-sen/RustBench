//! Sui Issue #2894: API Environment Loading Race
//!
//! This reproduces a race where multiple components try to load
//! configuration from storage concurrently, causing redundant loads
//! and potential inconsistent state.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/2894

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Environment {
    Development,
    Staging,
    Production,
}

/// Simulates persistent storage (file/database)
pub struct Storage {
    load_count: AtomicU64,
}

impl Storage {
    fn new() -> Self {
        Self {
            load_count: AtomicU64::new(0),
        }
    }

    fn load_api_environment(&self) -> Environment {
        // Simulate expensive I/O operation
        thread::sleep(Duration::from_millis(50));
        self.load_count.fetch_add(1, Ordering::SeqCst);
        println!(
            "  [STORAGE] Loading API environment from disk (load #{})",
            self.load_count.load(Ordering::SeqCst)
        );
        Environment::Production
    }

    fn get_load_count(&self) -> u64 {
        self.load_count.load(Ordering::SeqCst)
    }
}

/// Buggy application store - loads config on every access
mod buggy {
    use super::*;

    pub struct AppStore {
        storage: Arc<Storage>,
        // BUG: No cached state, loads on every access
        current_env: Mutex<Option<Environment>>,
    }

    impl AppStore {
        pub fn new(storage: Arc<Storage>) -> Self {
            Self {
                storage,
                current_env: Mutex::new(None),
            }
        }

        /// BUG: Race condition - multiple threads may load concurrently
        pub fn get_api_environment(&self) -> Environment {
            let mut env = self.current_env.lock().unwrap();

            // Check if loaded
            if env.is_none() {
                // Release lock while loading (simulating async behavior)
                drop(env);

                // RACE WINDOW: Multiple threads can reach here!
                println!("[BUGGY] Loading API environment...");
                let loaded = self.storage.load_api_environment();

                // Try to set it
                env = self.current_env.lock().unwrap();
                if env.is_none() {
                    *env = Some(loaded.clone());
                }
                loaded
            } else {
                env.clone().unwrap()
            }
        }

        /// Simulates multiple app components trying to get config
        pub fn initialize_component(&self, component_name: &str) {
            println!("[BUGGY] {} initializing...", component_name);
            let _env = self.get_api_environment();
            println!("[BUGGY] {} got environment", component_name);
        }
    }
}

/// Fixed application store - loads once and caches
mod fixed {
    use super::*;

    pub struct AppStore {
        storage: Arc<Storage>,
        // FIX: Initialize once and cache
        current_env: Mutex<Environment>,
    }

    impl AppStore {
        pub fn new(storage: Arc<Storage>) -> Self {
            println!("[FIXED] Initializing app with environment from storage...");
            let env = storage.load_api_environment();
            Self {
                storage,
                current_env: Mutex::new(env),
            }
        }

        /// FIX: Always use cached value, never reload
        pub fn get_api_environment(&self) -> Environment {
            let env = self.current_env.lock().unwrap();
            env.clone()
        }

        pub fn initialize_component(&self, component_name: &str) {
            println!("[FIXED] {} initializing...", component_name);
            let _env = self.get_api_environment();
            println!("[FIXED] {} got environment", component_name);
        }
    }
}

/// Alternative fix using Once
mod fixed_once {
    use super::*;

    pub struct AppStore {
        storage: Arc<Storage>,
        current_env: Mutex<Option<Environment>>,
        init_once: Once,
    }

    impl AppStore {
        pub fn new(storage: Arc<Storage>) -> Self {
            Self {
                storage,
                current_env: Mutex::new(None),
                init_once: Once::new(),
            }
        }

        /// FIX: Use Once to ensure single initialization
        pub fn get_api_environment(&self) -> Environment {
            self.init_once.call_once(|| {
                println!("[FIXED-ONCE] Loading API environment (one-time init)...");
                let env = self.storage.load_api_environment();
                let mut current = self.current_env.lock().unwrap();
                *current = Some(env);
            });

            let env = self.current_env.lock().unwrap();
            env.clone().unwrap()
        }

        pub fn initialize_component(&self, component_name: &str) {
            println!("[FIXED-ONCE] {} initializing...", component_name);
            let _env = self.get_api_environment();
            println!("[FIXED-ONCE] {} got environment", component_name);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");
    let use_once = args.iter().any(|arg| arg == "--once");

    println!("=== Sui Issue #2894: API Environment Loading Race ===\n");

    if use_once {
        println!("Running FIXED-ONCE version (std::sync::Once)...\n");
        run_fixed_once_test();
    } else if use_fixed {
        println!("Running FIXED version (load at init)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (lazy loading with race)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let storage = Arc::new(Storage::new());
    let store = Arc::new(buggy::AppStore::new(Arc::clone(&storage)));

    let mut handles = vec![];

    // Simulate 5 components initializing concurrently
    let components = vec!["UI", "API", "Wallet", "Network", "Storage"];

    for component in components {
        let store = Arc::clone(&store);
        let component = component.to_string();
        let handle = thread::spawn(move || {
            store.initialize_component(&component);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let load_count = storage.get_load_count();
    println!("\n=== Results ===");
    println!("Total storage loads: {}", load_count);

    if load_count > 1 {
        println!("\n[BUG DEMONSTRATED]");
        println!("Configuration was loaded {} times instead of once!", load_count);
        println!("Multiple threads raced to load the same configuration.");
        println!("This causes:");
        println!("  - Wasted I/O operations");
        println!("  - Potential inconsistent state");
        println!("  - Unnecessary resource usage");
    } else {
        println!("\n[NOTE]");
        println!("Race did not manifest this time (timing-dependent).");
        println!("Try running multiple times.");
    }

    println!("\nRun with --fixed to see load-at-init version.");
    println!("Run with --once to see std::sync::Once version.");
}

fn run_fixed_test() {
    let storage = Arc::new(Storage::new());
    let store = Arc::new(fixed::AppStore::new(Arc::clone(&storage)));

    println!();

    let mut handles = vec![];
    let components = vec!["UI", "API", "Wallet", "Network", "Storage"];

    for component in components {
        let store = Arc::clone(&store);
        let component = component.to_string();
        let handle = thread::spawn(move || {
            store.initialize_component(&component);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let load_count = storage.get_load_count();
    println!("\n=== Results ===");
    println!("Total storage loads: {}", load_count);
    println!("\n[FIXED]");
    println!("Configuration loaded exactly once during app initialization.");
    println!("All components reuse the cached value.");
}

fn run_fixed_once_test() {
    let storage = Arc::new(Storage::new());
    let store = Arc::new(fixed_once::AppStore::new(Arc::clone(&storage)));

    println!();

    let mut handles = vec![];
    let components = vec!["UI", "API", "Wallet", "Network", "Storage"];

    for component in components {
        let store = Arc::clone(&store);
        let component = component.to_string();
        let handle = thread::spawn(move || {
            store.initialize_component(&component);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let load_count = storage.get_load_count();
    println!("\n=== Results ===");
    println!("Total storage loads: {}", load_count);
    println!("\n[FIXED-ONCE]");
    println!("std::sync::Once ensures exactly-once initialization.");
    println!("First thread loads, others wait for completion.");
}
