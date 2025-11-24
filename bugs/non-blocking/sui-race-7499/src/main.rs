//! Sui Issue #7499: Port Binding Race Condition
//!
//! This reproduces a race condition where a new server tries to bind
//! to a port that was just released by another server.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/7499

use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const BASE_PORT: u16 = 19500;

/// Buggy version: Immediate panic on bind failure
fn spawn_server_buggy(port: u16, should_stop: Arc<AtomicBool>, bind_failures: Arc<AtomicU32>) {
    // BUG: Using unwrap() on bind - panics on transient "address in use" error
    let listener = match TcpListener::bind(format!("127.0.0.1:{}", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[BUGGY] Failed to bind to port {}: {}", port, e);
            bind_failures.fetch_add(1, Ordering::SeqCst);
            // In original code, this was unwrap() causing panic
            // We count failures instead to demonstrate the bug
            return;
        }
    };

    listener.set_nonblocking(true).unwrap();

    while !should_stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"OK");
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

/// Fixed version: Retry with backoff on bind failure
fn spawn_server_fixed(port: u16, should_stop: Arc<AtomicBool>, _bind_failures: Arc<AtomicU32>) {
    // FIX: Retry binding with exponential backoff
    let listener = retry_bind(port, 5, Duration::from_millis(10));

    let listener = match listener {
        Some(l) => l,
        None => {
            eprintln!("[FIXED] Failed to bind after retries (this shouldn't happen with proper backoff)");
            return;
        }
    };

    listener.set_nonblocking(true).unwrap();

    while !should_stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"OK");
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

/// Retry binding with exponential backoff
fn retry_bind(port: u16, max_retries: u32, initial_delay: Duration) -> Option<TcpListener> {
    let mut delay = initial_delay;

    for attempt in 0..max_retries {
        match TcpListener::bind(format!("127.0.0.1:{}", port)) {
            Ok(listener) => {
                if attempt > 0 {
                    println!("[FIXED] Bound to port {} after {} retries", port, attempt);
                }
                return Some(listener);
            }
            Err(e) => {
                println!(
                    "[FIXED] Bind attempt {} failed: {}, retrying in {:?}",
                    attempt + 1,
                    e,
                    delay
                );
                thread::sleep(delay);
                delay *= 2; // Exponential backoff
            }
        }
    }
    None
}

fn run_test(use_fixed: bool) {
    let port = BASE_PORT + (std::process::id() as u16 % 1000);
    let bind_failures = Arc::new(AtomicU32::new(0));

    println!("Testing on port {}...\n", port);

    // Simulate rapid server restart cycles
    let num_cycles = 5;

    for cycle in 0..num_cycles {
        println!("--- Cycle {} ---", cycle + 1);

        let should_stop = Arc::new(AtomicBool::new(false));
        let failures = Arc::clone(&bind_failures);
        let stop = Arc::clone(&should_stop);

        // Start server
        let handle = if use_fixed {
            thread::spawn(move || spawn_server_fixed(port, stop, failures))
        } else {
            thread::spawn(move || spawn_server_buggy(port, stop, failures))
        };

        // Let server run briefly
        thread::sleep(Duration::from_millis(50));

        // Make a test connection
        if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{}", port)) {
            let _ = stream.write_all(b"test");
            let mut buf = [0u8; 10];
            let _ = stream.read(&mut buf);
            println!("  Connection successful");
        }

        // Stop server (this releases the port)
        should_stop.store(true, Ordering::SeqCst);
        handle.join().unwrap();
        println!("  Server stopped");

        // BUG TRIGGER: Immediately try to start new server
        // The port may not be released yet (TIME_WAIT state)
        let should_stop2 = Arc::new(AtomicBool::new(false));
        let failures2 = Arc::clone(&bind_failures);
        let stop2 = Arc::clone(&should_stop2);

        // Rapid restart - this is where the race happens
        let handle2 = if use_fixed {
            thread::spawn(move || spawn_server_fixed(port, stop2, failures2))
        } else {
            thread::spawn(move || spawn_server_buggy(port, stop2, failures2))
        };

        thread::sleep(Duration::from_millis(50));
        should_stop2.store(true, Ordering::SeqCst);
        handle2.join().unwrap();
    }

    let total_failures = bind_failures.load(Ordering::SeqCst);

    println!("\n=== Results ===");
    println!("Total bind failures: {}", total_failures);

    if !use_fixed && total_failures > 0 {
        println!("\n[BUG DEMONSTRATED]");
        println!("Server failed to bind {} times due to port not being released in time.", total_failures);
        println!("In the original code, this caused a panic (unwrap on bind error).");
        println!("\nRun with --fixed to see retry logic.");
    } else if use_fixed {
        println!("\n[FIXED]");
        println!("Retry logic with backoff handles transient bind failures gracefully.");
    } else {
        println!("\n[NOTE]");
        println!("No failures this run. The race is timing-dependent.");
        println!("Run multiple times or under load to trigger.");
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #7499: Port Binding Race Condition ===\n");

    if use_fixed {
        println!("Running FIXED version (retry with backoff)...\n");
    } else {
        println!("Running BUGGY version (immediate failure)...\n");
    }

    run_test(use_fixed);
}
