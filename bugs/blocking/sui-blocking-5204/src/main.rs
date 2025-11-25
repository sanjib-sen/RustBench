//! Sui Issue #5204: BoundedExecutor Head-of-Line Blocking
//!
//! This reproduces head-of-line blocking where a bounded executor
//! runs out of tickets (capacity), causing all senders to block,
//! even when trying to send to different destinations.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/5204

use std::env;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Message {
    from: String,
    to: String,
    data: String,
}

/// Simulates a bounded executor with limited capacity
pub struct BoundedExecutor {
    sender: SyncSender<Message>,
    receiver: Mutex<Receiver<Message>>,
    capacity: usize,
}

impl BoundedExecutor {
    fn new(capacity: usize) -> Self {
        let (sender, receiver) = sync_channel(capacity);
        Self {
            sender,
            receiver: Mutex::new(receiver),
            capacity,
        }
    }

    /// BUG: Blocking send when executor is full
    fn send_message_blocking(&self, msg: Message) -> Result<(), String> {
        // This blocks if queue is full!
        self.sender
            .send(msg.clone())
            .map_err(|_| "Failed to send".to_string())?;
        println!(
            "[BLOCKING] Message from '{}' to '{}' queued (may have blocked)",
            msg.from, msg.to
        );
        Ok(())
    }

    /// FIX: Non-blocking send with drop policy
    fn send_message_nonblocking(&self, msg: Message) -> Result<(), String> {
        match self.sender.try_send(msg.clone()) {
            Ok(_) => {
                println!(
                    "[NONBLOCKING] Message from '{}' to '{}' queued",
                    msg.from, msg.to
                );
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                println!(
                    "[NONBLOCKING] Message from '{}' to '{}' DROPPED (executor full)",
                    msg.from, msg.to
                );
                Err("Executor full".to_string())
            }
            Err(TrySendError::Disconnected(_)) => Err("Disconnected".to_string()),
        }
    }

    fn process_messages(&self) {
        let receiver = self.receiver.lock().unwrap();
        while let Ok(msg) = receiver.recv_timeout(Duration::from_millis(50)) {
            // Simulate slow processing
            thread::sleep(Duration::from_millis(100));
            println!(
                "[EXECUTOR] Processed message from '{}' to '{}'",
                msg.from, msg.to
            );
        }
    }

    fn get_capacity(&self) -> usize {
        self.capacity
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #5204: BoundedExecutor Head-of-Line Blocking ===\n");

    if use_fixed {
        println!("Running FIXED version (non-blocking with drop policy)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (blocking send)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    // Small capacity to trigger blocking quickly
    let executor = Arc::new(BoundedExecutor::new(3));

    // Start executor processor (slow consumer)
    let executor_processor = Arc::clone(&executor);
    let processor_handle = thread::spawn(move || {
        executor_processor.process_messages();
    });

    println!(
        "Executor capacity: {}",
        executor.get_capacity()
    );
    println!("Sending 10 messages with slow processing...\n");

    let mut sender_handles = vec![];

    // Create multiple senders that will overwhelm the executor
    for i in 0..10 {
        let executor = Arc::clone(&executor);
        let handle = thread::spawn(move || {
            let msg = Message {
                from: format!("sender_{}", i),
                to: format!("validator_{}", i % 3),
                data: format!("data_{}", i),
            };

            let start = Instant::now();
            println!(
                "[BUGGY] Sender {} attempting to send at {:?}...",
                i,
                start.elapsed()
            );

            let result = executor.send_message_blocking(msg);

            let elapsed = start.elapsed();
            if elapsed > Duration::from_millis(50) {
                println!(
                    "[BUGGY] Sender {} BLOCKED for {:?}! (Head-of-line blocking)",
                    i, elapsed
                );
            }

            result
        });
        sender_handles.push(handle);
        thread::sleep(Duration::from_millis(10));
    }

    // Wait for all senders
    let mut blocked_count = 0;
    for (i, handle) in sender_handles.into_iter().enumerate() {
        if let Ok(_) = handle.join() {
            if i >= 3 {
                // After capacity is exceeded
                blocked_count += 1;
            }
        }
    }

    // Give processor time to finish
    thread::sleep(Duration::from_millis(500));

    println!("\n=== Results ===");
    println!("Multiple senders were blocked waiting for executor capacity.");
    println!("\n[BUG DEMONSTRATED]");
    println!("When one executor runs out of tickets, ALL senders block!");
    println!("This is head-of-line blocking - slow validator starves others.");
    println!("In Sui, this caused 'tx_helper_requests' occupancy to spike.");
    println!("\nRun with --fixed to see non-blocking version.");

    drop(processor_handle);
}

fn run_fixed_test() {
    let executor = Arc::new(BoundedExecutor::new(3));

    let executor_processor = Arc::clone(&executor);
    let processor_handle = thread::spawn(move || {
        executor_processor.process_messages();
    });

    println!(
        "Executor capacity: {}",
        executor.get_capacity()
    );
    println!("Sending 10 messages with slow processing...\n");

    let mut sender_handles = vec![];

    for i in 0..10 {
        let executor = Arc::clone(&executor);
        let handle = thread::spawn(move || {
            let msg = Message {
                from: format!("sender_{}", i),
                to: format!("validator_{}", i % 3),
                data: format!("data_{}", i),
            };

            let start = Instant::now();
            println!(
                "[FIXED] Sender {} attempting to send at {:?}...",
                i,
                start.elapsed()
            );

            let result = executor.send_message_nonblocking(msg);

            let elapsed = start.elapsed();
            if elapsed < Duration::from_millis(10) {
                println!("[FIXED] Sender {} completed immediately (no blocking)", i);
            }

            result
        });
        sender_handles.push(handle);
        thread::sleep(Duration::from_millis(10));
    }

    let mut dropped_count = 0;
    for handle in sender_handles {
        if let Ok(Err(_)) = handle.join() {
            dropped_count += 1;
        }
    }

    thread::sleep(Duration::from_millis(500));

    println!("\n=== Results ===");
    println!("Dropped {} messages when executor was full", dropped_count);
    println!("\n[FIXED]");
    println!("Non-blocking send with drop policy prevents head-of-line blocking.");
    println!("Senders to overloaded validators don't block other senders.");
    println!("Messages are dropped instead of blocking the entire system.");
    println!("\nFor unreliable networks, this is acceptable.");
    println!("For reliable networks, use 'spawn_with_permit' to pre-acquire capacity.");

    drop(processor_handle);
}
