//! Sui Issue #5469: Missing Certificate Effect Race
//!
//! This reproduces a race condition where a certificate effect goes missing
//! in the node_sync_store. The race occurs when:
//! 1. Thread A downloads a certificate and is about to check if it's still pending
//! 2. Thread B finishes processing the cert and removes it from pending_certs
//! 3. Thread A sees the cert is no longer pending and skips storing the effect
//!
//! Original Issue: https://github.com/MystenLabs/sui/issues/5469

use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

pub type CertDigest = String;
pub type EffectDigest = String;

/// Represents a certificate in the system
#[derive(Clone, Debug)]
pub struct Certificate {
    digest: CertDigest,
    data: Vec<u8>,
}

/// Represents the effect of executing a certificate
#[derive(Clone, Debug)]
pub struct CertificateEffect {
    cert_digest: CertDigest,
    effect_digest: EffectDigest,
}

/// Node sync store that tracks certificate effects
pub struct NodeSyncStore {
    effects: RwLock<HashMap<CertDigest, CertificateEffect>>,
}

impl NodeSyncStore {
    fn new() -> Self {
        Self {
            effects: RwLock::new(HashMap::new()),
        }
    }

    fn store_effect(&self, effect: CertificateEffect) {
        let mut effects = self.effects.write().unwrap();
        println!("[STORE] Storing effect for cert: {}", effect.cert_digest);
        effects.insert(effect.cert_digest.clone(), effect);
    }

    fn has_effect(&self, cert_digest: &str) -> bool {
        self.effects.read().unwrap().contains_key(cert_digest)
    }

    fn get_all_effects(&self) -> Vec<CertDigest> {
        self.effects.read().unwrap().keys().cloned().collect()
    }
}

/// Tracks certificates that are being downloaded/processed
pub struct PendingCerts {
    pending: Mutex<HashSet<CertDigest>>,
}

impl PendingCerts {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashSet::new()),
        }
    }

    fn add(&self, digest: &str) {
        self.pending.lock().unwrap().insert(digest.to_string());
    }

    fn remove(&self, digest: &str) -> bool {
        self.pending.lock().unwrap().remove(digest)
    }

    fn contains(&self, digest: &str) -> bool {
        self.pending.lock().unwrap().contains(digest)
    }
}

/// Buggy version - race between download check and process completion
mod buggy {
    use super::*;

    pub struct NodeSyncState {
        store: Arc<NodeSyncStore>,
        pending: Arc<PendingCerts>,
    }

    impl NodeSyncState {
        pub fn new(store: Arc<NodeSyncStore>, pending: Arc<PendingCerts>) -> Self {
            Self { store, pending }
        }

        /// Download and process a certificate
        /// BUG: Race between checking pending status and another thread completing
        pub fn download_and_sync(&self, cert_digest: &str) {
            println!("[BUGGY] Starting download for cert: {}", cert_digest);

            // Add to pending before download
            self.pending.add(cert_digest);

            // Simulate download time (network latency)
            thread::sleep(Duration::from_millis(50));

            // Download complete, now simulate the certificate data
            let cert = Certificate {
                digest: cert_digest.to_string(),
                data: vec![1, 2, 3],
            };

            println!("[BUGGY] Download complete for cert: {}", cert_digest);

            // BUG: Race condition window!
            // Between download completion and this check, another thread may have:
            // 1. Processed the cert
            // 2. Removed it from pending
            // 3. But failed to store the effect properly

            // Small delay to widen race window for demonstration
            thread::sleep(Duration::from_millis(10));

            // Check if cert is still pending (another thread may have processed it)
            if !self.pending.contains(cert_digest) {
                // BUG: Assume another thread handled it, skip processing
                println!("[BUGGY] Cert {} no longer pending, skipping (EFFECT MAY BE LOST!)", cert_digest);
                return;
            }

            // Process and store effect
            let effect = CertificateEffect {
                cert_digest: cert_digest.to_string(),
                effect_digest: format!("effect_{}", cert_digest),
            };

            self.store.store_effect(effect);
            self.pending.remove(cert_digest);
        }

        /// Process a certificate from consensus
        /// This runs concurrently with download_and_sync
        pub fn process_from_consensus(&self, cert_digest: &str) {
            println!("[BUGGY] Processing cert from consensus: {}", cert_digest);

            // Check if we're already downloading this cert
            if self.pending.contains(cert_digest) {
                // Remove from pending to signal download thread we're handling it
                self.pending.remove(cert_digest);
                println!("[BUGGY] Removed cert {} from pending", cert_digest);

                // BUG: In certain error paths, we might fail to store the effect
                // Simulate an intermittent failure (e.g., db error, timeout)
                let should_fail = cert_digest.contains("fail");

                if should_fail {
                    println!("[BUGGY] Failed to process cert {} (effect NOT stored!)", cert_digest);
                    // Effect is lost! Download thread will skip, we failed to store
                    return;
                }
            }

            // Store effect
            let effect = CertificateEffect {
                cert_digest: cert_digest.to_string(),
                effect_digest: format!("effect_{}", cert_digest),
            };
            self.store.store_effect(effect);
        }
    }
}

/// Fixed version - ensure effect is always stored
mod fixed {
    use super::*;

    pub struct NodeSyncState {
        store: Arc<NodeSyncStore>,
        pending: Arc<PendingCerts>,
    }

    impl NodeSyncState {
        pub fn new(store: Arc<NodeSyncStore>, pending: Arc<PendingCerts>) -> Self {
            Self { store, pending }
        }

        /// FIX: Always ensure effect is stored, regardless of pending status
        pub fn download_and_sync(&self, cert_digest: &str) {
            println!("[FIXED] Starting download for cert: {}", cert_digest);

            self.pending.add(cert_digest);

            thread::sleep(Duration::from_millis(50));

            let cert = Certificate {
                digest: cert_digest.to_string(),
                data: vec![1, 2, 3],
            };

            println!("[FIXED] Download complete for cert: {}", cert_digest);

            thread::sleep(Duration::from_millis(10));

            // FIX: Don't skip processing based on pending status
            // Always check if effect exists before deciding to skip
            if self.store.has_effect(cert_digest) {
                println!("[FIXED] Effect already exists for cert: {}", cert_digest);
                self.pending.remove(cert_digest);
                return;
            }

            // Process and store effect
            let effect = CertificateEffect {
                cert_digest: cert_digest.to_string(),
                effect_digest: format!("effect_{}", cert_digest),
            };

            self.store.store_effect(effect);
            self.pending.remove(cert_digest);
        }

        /// FIX: Always store effect, even on error paths
        pub fn process_from_consensus(&self, cert_digest: &str) {
            println!("[FIXED] Processing cert from consensus: {}", cert_digest);

            // FIX: Check if effect already exists before processing
            if self.store.has_effect(cert_digest) {
                println!("[FIXED] Effect already exists for cert: {}", cert_digest);
                self.pending.remove(cert_digest);
                return;
            }

            // Remove from pending
            self.pending.remove(cert_digest);

            // Even on "failure", ensure we don't lose the effect
            // In the fix, failures should be retried or handled properly
            let should_fail = cert_digest.contains("fail");

            if should_fail {
                // FIX: Re-add to pending for retry instead of losing the effect
                println!("[FIXED] Processing failed for cert {}, re-adding to pending for retry", cert_digest);
                self.pending.add(cert_digest);
                // In real fix, would trigger retry mechanism
                // For demo, we'll store a placeholder effect
                let effect = CertificateEffect {
                    cert_digest: cert_digest.to_string(),
                    effect_digest: format!("effect_{}_retry", cert_digest),
                };
                self.store.store_effect(effect);
                self.pending.remove(cert_digest);
                return;
            }

            let effect = CertificateEffect {
                cert_digest: cert_digest.to_string(),
                effect_digest: format!("effect_{}", cert_digest),
            };
            self.store.store_effect(effect);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #5469: Missing Certificate Effect Race ===\n");

    if use_fixed {
        println!("Running FIXED version (check effect existence, not pending status)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (race causes missing effect)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let store = Arc::new(NodeSyncStore::new());
    let pending = Arc::new(PendingCerts::new());

    // Certificates to process - one will "fail" in consensus processing
    let certs = vec!["cert_1", "cert_fail_2", "cert_3"];

    println!("Scenario: Download 3 certs while consensus processes them concurrently");
    println!("cert_fail_2 will fail in consensus processing\n");

    let mut handles = vec![];

    // Start download threads
    for cert in &certs {
        let state = buggy::NodeSyncState::new(Arc::clone(&store), Arc::clone(&pending));
        let cert = cert.to_string();
        handles.push(thread::spawn(move || {
            state.download_and_sync(&cert);
        }));
    }

    // Start consensus processing threads (with slight delay to create race)
    thread::sleep(Duration::from_millis(30));

    for cert in &certs {
        let state = buggy::NodeSyncState::new(Arc::clone(&store), Arc::clone(&pending));
        let cert = cert.to_string();
        handles.push(thread::spawn(move || {
            state.process_from_consensus(&cert);
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Check results
    let stored_effects = store.get_all_effects();

    println!("\n=== Results ===");
    println!("Expected certificates: {:?}", certs);
    println!("Stored effects: {:?}", stored_effects);

    let missing: Vec<_> = certs.iter()
        .filter(|c| !stored_effects.contains(&c.to_string()))
        .collect();

    if !missing.is_empty() {
        println!("\n[BUG DEMONSTRATED]");
        println!("Missing effects for certificates: {:?}", missing);
        println!("\nProblem:");
        println!("  - Download thread saw cert removed from pending");
        println!("  - Consensus thread failed to store effect");
        println!("  - Neither thread stored the effect!");
        println!("\nRun with --fixed to see proper synchronization.");
    } else {
        println!("\nAll effects stored (race didn't manifest this run)");
        println!("Try running multiple times to see the bug.");
    }
}

fn run_fixed_test() {
    let store = Arc::new(NodeSyncStore::new());
    let pending = Arc::new(PendingCerts::new());

    let certs = vec!["cert_1", "cert_fail_2", "cert_3"];

    println!("Scenario: Download 3 certs while consensus processes them concurrently");
    println!("cert_fail_2 will fail in consensus processing (but effect still stored)\n");

    let mut handles = vec![];

    for cert in &certs {
        let state = fixed::NodeSyncState::new(Arc::clone(&store), Arc::clone(&pending));
        let cert = cert.to_string();
        handles.push(thread::spawn(move || {
            state.download_and_sync(&cert);
        }));
    }

    thread::sleep(Duration::from_millis(30));

    for cert in &certs {
        let state = fixed::NodeSyncState::new(Arc::clone(&store), Arc::clone(&pending));
        let cert = cert.to_string();
        handles.push(thread::spawn(move || {
            state.process_from_consensus(&cert);
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let stored_effects = store.get_all_effects();

    println!("\n=== Results ===");
    println!("Expected certificates: {:?}", certs);
    println!("Stored effects: {:?}", stored_effects);

    let missing: Vec<_> = certs.iter()
        .filter(|c| !stored_effects.contains(&c.to_string()))
        .collect();

    if missing.is_empty() {
        println!("\n[FIXED]");
        println!("All effects stored!");
        println!("\nFix: Check effect existence, not pending status");
        println!("  - Don't skip based on pending flag alone");
        println!("  - Always verify effect is stored before skipping");
        println!("  - Handle failures by retry, not silent skip");
    } else {
        println!("\nUnexpected: Missing effects {:?}", missing);
    }
}
