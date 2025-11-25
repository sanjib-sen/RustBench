//! Sui PR #5868: Batch Notifier Missing Notification on Failure
//!
//! This reproduces a blocking bug where the batch notifier is not notified
//! when a transaction is sequenced but the commit fails. This causes
//! dependent operations to block forever waiting for the notification.
//!
//! Original PR: https://github.com/MystenLabs/sui/pull/5868

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

pub type SequenceNumber = u64;
pub type TxDigest = String;

/// Simulates the batch notifier that tracks sequence numbers
/// This version requires contiguous sequences (no gaps)
pub struct BatchNotifier {
    /// Tracks sequences that have been notified
    notified_sequences: Mutex<Vec<SequenceNumber>>,
    /// The next expected sequence for contiguous consumption
    next_expected: Mutex<SequenceNumber>,
    /// Condition variable for waiting on sequence numbers
    notify: Condvar,
}

impl BatchNotifier {
    fn new() -> Self {
        Self {
            notified_sequences: Mutex::new(Vec::new()),
            next_expected: Mutex::new(1),
            notify: Condvar::new(),
        }
    }

    fn notify_sequence(&self, seq: SequenceNumber) {
        let mut sequences = self.notified_sequences.lock().unwrap();
        sequences.push(seq);
        sequences.sort();
        println!("[NOTIFIER] Notified sequence {}", seq);
        self.notify.notify_all();
    }

    /// Wait for contiguous sequences up to target
    /// Returns false if there's a gap in the sequence
    fn wait_for_contiguous(&self, target: SequenceNumber, timeout: Duration) -> bool {
        let start = std::time::Instant::now();

        loop {
            {
                let sequences = self.notified_sequences.lock().unwrap();
                let next_expected = self.next_expected.lock().unwrap();

                // Check if we have contiguous sequences up to target
                let mut current = *next_expected;
                for &seq in sequences.iter() {
                    if seq == current {
                        current += 1;
                    }
                }

                if current > target {
                    return true;
                }
            }

            if start.elapsed() >= timeout {
                return false;
            }

            let sequences = self.notified_sequences.lock().unwrap();
            let remaining = timeout.saturating_sub(start.elapsed());
            let _ = self.notify.wait_timeout(sequences, remaining);
        }
    }

    fn get_notified(&self) -> Vec<SequenceNumber> {
        self.notified_sequences.lock().unwrap().clone()
    }
}

/// Database for storing committed transactions
pub struct Database {
    committed: Mutex<HashMap<TxDigest, SequenceNumber>>,
    should_fail: Mutex<bool>,
}

impl Database {
    fn new() -> Self {
        Self {
            committed: Mutex::new(HashMap::new()),
            should_fail: Mutex::new(false),
        }
    }

    fn set_fail(&self, fail: bool) {
        *self.should_fail.lock().unwrap() = fail;
    }

    fn commit(&self, digest: &str, seq: SequenceNumber) -> Result<(), &'static str> {
        if *self.should_fail.lock().unwrap() {
            return Err("Database commit failed");
        }
        let mut committed = self.committed.lock().unwrap();
        committed.insert(digest.to_string(), seq);
        Ok(())
    }
}

/// Buggy version - doesn't notify on commit failure
mod buggy {
    use super::*;

    pub struct Authority {
        notifier: Arc<BatchNotifier>,
        database: Arc<Database>,
        next_seq: Mutex<SequenceNumber>,
    }

    impl Authority {
        pub fn new(notifier: Arc<BatchNotifier>, database: Arc<Database>) -> Self {
            Self {
                notifier,
                database,
                next_seq: Mutex::new(1),
            }
        }

        /// BUG: If commit fails after sequencing, notifier is never updated
        pub fn commit_certificate(&self, digest: &str) -> Result<SequenceNumber, &'static str> {
            // Step 1: Assign sequence number
            let seq = {
                let mut next = self.next_seq.lock().unwrap();
                let seq = *next;
                *next += 1;
                seq
            };
            println!("[BUGGY] Assigned sequence {} to {}", seq, digest);

            // Step 2: Try to commit to database
            match self.database.commit(digest, seq) {
                Ok(()) => {
                    // Only notify on success
                    self.notifier.notify_sequence(seq);
                    Ok(seq)
                }
                Err(e) => {
                    // BUG: Sequence was assigned but not notified!
                    println!("[BUGGY] Commit failed for {} (seq {}), NOT notifying!", digest, seq);
                    Err(e)
                }
            }
        }

        pub fn notifier(&self) -> &Arc<BatchNotifier> {
            &self.notifier
        }
    }
}

/// Fixed version - notifies even on commit failure
mod fixed {
    use super::*;

    pub struct Authority {
        notifier: Arc<BatchNotifier>,
        database: Arc<Database>,
        next_seq: Mutex<SequenceNumber>,
    }

    impl Authority {
        pub fn new(notifier: Arc<BatchNotifier>, database: Arc<Database>) -> Self {
            Self {
                notifier,
                database,
                next_seq: Mutex::new(1),
            }
        }

        /// FIX: Always notify the batch notifier, even on failure
        pub fn commit_certificate(&self, digest: &str) -> Result<SequenceNumber, &'static str> {
            let seq = {
                let mut next = self.next_seq.lock().unwrap();
                let seq = *next;
                *next += 1;
                seq
            };
            println!("[FIXED] Assigned sequence {} to {}", seq, digest);

            let result = self.database.commit(digest, seq);

            // FIX: Always notify the sequence number was used
            // This prevents blocking even if commit failed
            self.notifier.notify_sequence(seq);

            match result {
                Ok(()) => {
                    println!("[FIXED] Commit succeeded for {} (seq {})", digest, seq);
                    Ok(seq)
                }
                Err(e) => {
                    println!("[FIXED] Commit failed for {} (seq {}), but notified anyway", digest, seq);
                    Err(e)
                }
            }
        }

        pub fn notifier(&self) -> &Arc<BatchNotifier> {
            &self.notifier
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui PR #5868: Batch Notifier Missing Notification ===\n");

    if use_fixed {
        println!("Running FIXED version (always notify)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (missing notification on failure)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let notifier = Arc::new(BatchNotifier::new());
    let database = Arc::new(Database::new());
    let authority = Arc::new(buggy::Authority::new(
        Arc::clone(&notifier),
        Arc::clone(&database),
    ));

    println!("Scenario: Commit tx1 (success), tx2 (fail), tx3 (success)");
    println!("Problem: Gap in sequence chain blocks contiguous consumption\n");

    // First commit succeeds
    let _ = authority.commit_certificate("tx1");

    // Make database fail for next commit
    database.set_fail(true);

    // Second commit fails - sequence 2 is assigned but not notified!
    let _ = authority.commit_certificate("tx2");

    // Re-enable database
    database.set_fail(false);

    // Third commit succeeds
    let _ = authority.commit_certificate("tx3");

    println!("\nNotified sequences: {:?}", notifier.get_notified());
    println!("(Missing sequence 2 creates a gap!)\n");

    // Now try to wait for contiguous sequences up to 3
    println!("Waiting for contiguous sequences 1,2,3 (2 second timeout)...");
    let notifier_clone = Arc::clone(&notifier);
    let handle = thread::spawn(move || {
        notifier_clone.wait_for_contiguous(3, Duration::from_secs(2))
    });

    let got_contiguous = handle.join().unwrap();

    println!("\n=== Results ===");
    if !got_contiguous {
        println!("[BUG DEMONSTRATED]");
        println!("Wait for contiguous sequences timed out!");
        println!("\nProblem:");
        println!("  - tx2 was assigned sequence 2");
        println!("  - tx2's commit failed");
        println!("  - Notifier has [1, 3] but missing 2!");
        println!("  - Can't proceed without contiguous chain");
        println!("\nRun with --fixed to see proper notification.");
    } else {
        println!("Got contiguous sequences (unexpected in buggy version)");
    }
}

fn run_fixed_test() {
    let notifier = Arc::new(BatchNotifier::new());
    let database = Arc::new(Database::new());
    let authority = Arc::new(fixed::Authority::new(
        Arc::clone(&notifier),
        Arc::clone(&database),
    ));

    println!("Scenario: Commit tx1 (success), tx2 (fail), tx3 (success)");
    println!("Fix: Always notify sequence, even on failure\n");

    let _ = authority.commit_certificate("tx1");

    database.set_fail(true);
    let _ = authority.commit_certificate("tx2");

    database.set_fail(false);
    let _ = authority.commit_certificate("tx3");

    println!("\nNotified sequences: {:?}", notifier.get_notified());

    println!("\nWaiting for contiguous sequences 1,2,3...");
    let notifier_clone = Arc::clone(&notifier);
    let handle = thread::spawn(move || {
        notifier_clone.wait_for_contiguous(3, Duration::from_secs(2))
    });

    let got_contiguous = handle.join().unwrap();

    println!("\n=== Results ===");
    if got_contiguous {
        println!("[FIXED]");
        println!("Got contiguous sequences [1, 2, 3]!");
        println!("\nFix: Notify batch notifier even on commit failure");
        println!("  - Sequence numbers are always reported");
        println!("  - No gaps in the sequence chain");
        println!("  - System maintains progress");
    } else {
        println!("Unexpected timeout");
    }
}
