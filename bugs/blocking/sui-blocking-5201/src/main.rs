//! Sui Issue #5201: Bounded Queue Deadlock
//!
//! This reproduces a deadlock where a bounded queue fills up during
//! recursive processing, blocking producers and causing system deadlock.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/5201

use std::collections::VecDeque;
use std::env;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const QUEUE_CAPACITY: usize = 10; // Small capacity to demonstrate bug quickly

#[derive(Debug, Clone)]
struct Certificate {
    id: u64,
    parent_id: Option<u64>, // Dependency on parent certificate
}

/// Buggy version: Uses bounded synchronous channel
mod buggy {
    use super::*;

    pub struct CertificateWaiter {
        // Bounded queue - this is the problem
        sender: SyncSender<Certificate>,
        receiver: Arc<Mutex<Receiver<Certificate>>>,
        processed: Arc<Mutex<Vec<u64>>>,
    }

    impl CertificateWaiter {
        pub fn new() -> Self {
            let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
            Self {
                sender,
                receiver: Arc::new(Mutex::new(receiver)),
                processed: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Process a certificate - may trigger recursive fetching
        pub fn process_certificate(&self, cert: Certificate) -> bool {
            println!("[BUGGY] Processing certificate {}", cert.id);

            // Check if we need to fetch parent first
            if let Some(parent_id) = cert.parent_id {
                let processed = self.processed.lock().unwrap();
                if !processed.contains(&parent_id) {
                    drop(processed);

                    // Queue this certificate to wait for parent
                    println!(
                        "[BUGGY] Cert {} waiting for parent {}, queuing...",
                        cert.id, parent_id
                    );

                    // BUG: This blocks if queue is full!
                    // In real system, this causes deadlock when:
                    // 1. Queue fills with certs waiting for parents
                    // 2. Processing parent triggers more certs
                    // 3. New certs can't be queued -> blocked forever
                    match self.sender.try_send(cert.clone()) {
                        Ok(_) => {
                            // Simulate fetching parent (triggers recursive processing)
                            self.fetch_parent(parent_id);
                        }
                        Err(TrySendError::Full(_)) => {
                            println!(
                                "[BUGGY] QUEUE FULL! Cannot queue cert {} - DEADLOCK!",
                                cert.id
                            );
                            return false;
                        }
                        Err(TrySendError::Disconnected(_)) => return false,
                    }
                    return true;
                }
            }

            // Process the certificate
            let mut processed = self.processed.lock().unwrap();
            processed.push(cert.id);
            println!("[BUGGY] Cert {} processed successfully", cert.id);
            true
        }

        /// Fetch a parent certificate - this triggers more processing
        fn fetch_parent(&self, parent_id: u64) {
            println!("[BUGGY] Fetching parent certificate {}", parent_id);

            // Simulate network delay
            thread::sleep(Duration::from_millis(10));

            // Parent certificate may also have a parent (recursive!)
            let grandparent = if parent_id > 1 {
                Some(parent_id - 1)
            } else {
                None
            };

            let parent_cert = Certificate {
                id: parent_id,
                parent_id: grandparent,
            };

            // Recursive call - this is where deadlock happens
            self.process_certificate(parent_cert);
        }

        /// Consumer thread - processes waiting certificates
        pub fn run_consumer(&self) {
            loop {
                let cert = {
                    let receiver = self.receiver.lock().unwrap();
                    match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(cert) => cert,
                        Err(_) => break,
                    }
                };

                // Re-process after parent should be ready
                thread::sleep(Duration::from_millis(50)); // Slow consumer
                self.process_certificate(cert);
            }
        }
    }
}

/// Fixed version: Uses unbounded queue with backpressure
mod fixed {
    use super::*;

    pub struct CertificateWaiter {
        // Unbounded queue - won't block producer
        queue: Arc<Mutex<VecDeque<Certificate>>>,
        processed: Arc<Mutex<Vec<u64>>>,
        pending_count: Arc<Mutex<usize>>,
    }

    impl CertificateWaiter {
        pub fn new() -> Self {
            Self {
                queue: Arc::new(Mutex::new(VecDeque::new())),
                processed: Arc::new(Mutex::new(Vec::new())),
                pending_count: Arc::new(Mutex::new(0)),
            }
        }

        pub fn process_certificate(&self, cert: Certificate) -> bool {
            println!("[FIXED] Processing certificate {}", cert.id);

            if let Some(parent_id) = cert.parent_id {
                let processed = self.processed.lock().unwrap();
                if !processed.contains(&parent_id) {
                    drop(processed);

                    println!(
                        "[FIXED] Cert {} waiting for parent {}, queuing...",
                        cert.id, parent_id
                    );

                    // FIX: Unbounded queue - never blocks
                    {
                        let mut queue = self.queue.lock().unwrap();
                        queue.push_back(cert.clone());
                        let mut count = self.pending_count.lock().unwrap();
                        *count += 1;
                        println!("[FIXED] Queue size: {}", queue.len());
                    }

                    // Fetch parent
                    self.fetch_parent(parent_id);
                    return true;
                }
            }

            let mut processed = self.processed.lock().unwrap();
            processed.push(cert.id);
            println!("[FIXED] Cert {} processed successfully", cert.id);
            true
        }

        fn fetch_parent(&self, parent_id: u64) {
            println!("[FIXED] Fetching parent certificate {}", parent_id);
            thread::sleep(Duration::from_millis(10));

            let grandparent = if parent_id > 1 {
                Some(parent_id - 1)
            } else {
                None
            };

            let parent_cert = Certificate {
                id: parent_id,
                parent_id: grandparent,
            };

            self.process_certificate(parent_cert);
        }

        pub fn run_consumer(&self) {
            loop {
                let cert = {
                    let mut queue = self.queue.lock().unwrap();
                    queue.pop_front()
                };

                match cert {
                    Some(c) => {
                        thread::sleep(Duration::from_millis(50));
                        self.process_certificate(c);
                    }
                    None => {
                        thread::sleep(Duration::from_millis(10));
                        let count = self.pending_count.lock().unwrap();
                        if *count == 0 {
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #5201: Bounded Queue Deadlock ===\n");

    if use_fixed {
        println!("Running FIXED version (unbounded queue)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (bounded queue, capacity={})...\n", QUEUE_CAPACITY);
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let waiter = Arc::new(buggy::CertificateWaiter::new());
    let waiter_consumer = Arc::clone(&waiter);

    // Start consumer
    let consumer_handle = thread::spawn(move || {
        waiter_consumer.run_consumer();
    });

    // Producer: Send certificates with deep dependency chains
    // Certificate 15 depends on 14, which depends on 13, etc.
    // This creates a deep chain that fills the bounded queue
    println!("Sending certificates with deep dependency chain...\n");

    for i in (5..20).rev() {
        let cert = Certificate {
            id: i,
            parent_id: if i > 1 { Some(i - 1) } else { None },
        };

        println!("--- Sending certificate {} ---", i);
        waiter.process_certificate(cert);

        // Small delay between sends
        thread::sleep(Duration::from_millis(5));
    }

    // Wait a bit for processing
    thread::sleep(Duration::from_secs(1));
    drop(waiter); // Close the channel
    let _ = consumer_handle.join();

    println!("\n[BUG DEMONSTRATED]");
    println!("The bounded queue filled up, causing some certificates to be rejected.");
    println!("In the real system, this causes deadlock as the entire chain stalls.");
    println!("\nRun with --fixed to see unbounded queue handling.");
}

fn run_fixed_test() {
    let waiter = Arc::new(fixed::CertificateWaiter::new());
    let waiter_consumer = Arc::clone(&waiter);

    let consumer_handle = thread::spawn(move || {
        waiter_consumer.run_consumer();
    });

    println!("Sending certificates with deep dependency chain...\n");

    for i in (5..20).rev() {
        let cert = Certificate {
            id: i,
            parent_id: if i > 1 { Some(i - 1) } else { None },
        };

        println!("--- Sending certificate {} ---", i);
        waiter.process_certificate(cert);
        thread::sleep(Duration::from_millis(5));
    }

    thread::sleep(Duration::from_secs(2));
    let _ = consumer_handle.join();

    println!("\n[FIXED]");
    println!("Unbounded queue allows all certificates to be queued.");
    println!("Processing continues without deadlock.");
}
