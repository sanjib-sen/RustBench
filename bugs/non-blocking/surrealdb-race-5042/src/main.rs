//! SurrealDB Issue #5042: Concurrent Authentication Race
//!
//! This reproduces a race where concurrent authentication requests
//! try to UPDATE $auth SET lastActive = time::now() simultaneously,
//! causing authentication failures for all but the first request.
//!
//! Original bug: https://github.com/surrealdb/surrealdb/issues/5042

use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub struct AuthToken {
    user_id: String,
    last_active: SystemTime,
}

#[derive(Debug, Clone)]
pub enum AuthResult {
    Success,
    AuthenticationFailed(String),
}

/// Simulates authentication storage
pub struct AuthStore {
    tokens: RwLock<HashMap<String, AuthToken>>,
    failed_auth_count: AtomicU64,
}

impl AuthStore {
    fn new() -> Self {
        let mut tokens = HashMap::new();
        tokens.insert(
            "token_123".to_string(),
            AuthToken {
                user_id: "alice".to_string(),
                last_active: SystemTime::now(),
            },
        );
        Self {
            tokens: RwLock::new(tokens),
            failed_auth_count: AtomicU64::new(0),
        }
    }

    fn get_failed_count(&self) -> u64 {
        self.failed_auth_count.load(Ordering::SeqCst)
    }
}

/// Buggy authentication handler - non-atomic read-update
mod buggy {
    use super::*;

    pub struct AuthHandler {
        store: Arc<AuthStore>,
    }

    impl AuthHandler {
        pub fn new(store: Arc<AuthStore>) -> Self {
            Self { store }
        }

        /// BUG: Non-atomic read-modify-write on auth record
        pub fn authenticate(&self, token: &str) -> AuthResult {
            // Step 1: Read token (check if valid)
            let token_data = {
                let tokens = self.store.tokens.read().unwrap();
                match tokens.get(token) {
                    Some(data) => data.clone(),
                    None => {
                        return AuthResult::AuthenticationFailed("Invalid token".to_string())
                    }
                }
            };
            // Lock released here!

            // BUG: Race window! Another thread can also pass validation
            thread::sleep(Duration::from_micros(100)); // Simulate processing

            // Step 2: Update lastActive (write)
            {
                let mut tokens = self.store.tokens.write().unwrap();

                // Check again if token still exists (defensive check)
                if let Some(auth_token) = tokens.get_mut(token) {
                    // Simulate: UPDATE $auth SET lastActive = time::now()
                    let new_time = SystemTime::now();

                    // BUG: If another thread also reached here, one update
                    // will fail or they'll conflict
                    if auth_token.last_active > new_time.checked_sub(Duration::from_millis(50)).unwrap() {
                        // Another thread just updated this!
                        self.store
                            .failed_auth_count
                            .fetch_add(1, Ordering::SeqCst);
                        println!(
                            "[BUGGY] Authentication FAILED for token '{}' - concurrent update detected",
                            token
                        );
                        return AuthResult::AuthenticationFailed(
                            "Concurrent authentication conflict".to_string(),
                        );
                    }

                    auth_token.last_active = new_time;
                    println!(
                        "[BUGGY] Authentication SUCCESS for token '{}' by user '{}'",
                        token, token_data.user_id
                    );
                    AuthResult::Success
                } else {
                    AuthResult::AuthenticationFailed("Token disappeared".to_string())
                }
            }
        }
    }
}

/// Fixed authentication handler - atomic operation
mod fixed {
    use super::*;

    pub struct AuthHandler {
        store: Arc<AuthStore>,
    }

    impl AuthHandler {
        pub fn new(store: Arc<AuthStore>) -> Self {
            Self { store }
        }

        /// FIX: Hold write lock for entire validate-and-update sequence
        pub fn authenticate(&self, token: &str) -> AuthResult {
            // Hold write lock for entire operation
            let mut tokens = self.store.tokens.write().unwrap();

            match tokens.get_mut(token) {
                Some(auth_token) => {
                    // Atomically validate and update
                    auth_token.last_active = SystemTime::now();
                    println!(
                        "[FIXED] Authentication SUCCESS for token '{}' by user '{}'",
                        token, auth_token.user_id
                    );
                    AuthResult::Success
                }
                None => {
                    println!("[FIXED] Authentication FAILED - invalid token");
                    AuthResult::AuthenticationFailed("Invalid token".to_string())
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== SurrealDB Issue #5042: Concurrent Authentication Race ===\n");

    if use_fixed {
        println!("Running FIXED version (atomic validate-and-update)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (racy read-then-update)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let store = Arc::new(AuthStore::new());
    let handler = Arc::new(buggy::AuthHandler::new(Arc::clone(&store)));

    println!("Simulating 10 concurrent authentication requests...\n");

    let mut handles = vec![];

    // Simulate 10 concurrent requests with the same token
    for i in 0..10 {
        let handler = Arc::clone(&handler);
        let handle = thread::spawn(move || {
            println!("[BUGGY] Request {} authenticating...", i);
            handler.authenticate("token_123")
        });
        handles.push(handle);
    }

    let mut success_count = 0;
    let mut failed_count = 0;

    for handle in handles {
        match handle.join().unwrap() {
            AuthResult::Success => success_count += 1,
            AuthResult::AuthenticationFailed(_) => failed_count += 1,
        }
    }

    println!("\n=== Results ===");
    println!("Successful authentications: {}", success_count);
    println!("Failed authentications: {}", failed_count);

    if failed_count > 0 {
        println!("\n[BUG DEMONSTRATED]");
        println!("Only {} out of 10 requests succeeded!", success_count);
        println!("Concurrent UPDATE $auth SET lastActive caused race conflicts.");
        println!("In production, this caused 'There was a problem with authentication' errors.");
    } else {
        println!("\n[NOTE]");
        println!("All requests succeeded this time (timing-dependent race).");
        println!("Try running multiple times.");
    }

    println!("\nRun with --fixed to see atomic version.");
}

fn run_fixed_test() {
    let store = Arc::new(AuthStore::new());
    let handler = Arc::new(fixed::AuthHandler::new(Arc::clone(&store)));

    println!("Simulating 10 concurrent authentication requests...\n");

    let mut handles = vec![];

    for i in 0..10 {
        let handler = Arc::clone(&handler);
        let handle = thread::spawn(move || {
            println!("[FIXED] Request {} authenticating...", i);
            handler.authenticate("token_123")
        });
        handles.push(handle);
    }

    let mut success_count = 0;
    let mut failed_count = 0;

    for handle in handles {
        match handle.join().unwrap() {
            AuthResult::Success => success_count += 1,
            AuthResult::AuthenticationFailed(_) => failed_count += 1,
        }
    }

    println!("\n=== Results ===");
    println!("Successful authentications: {}", success_count);
    println!("Failed authentications: {}", failed_count);

    if success_count == 10 {
        println!("\n[FIXED]");
        println!("All 10 requests succeeded!");
        println!("Atomic validate-and-update prevents race condition.");
        println!("Write lock held during entire authentication sequence.");
    }
}
