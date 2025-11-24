//! Sui Issue #8113: Concurrent Build Directory Race
//!
//! This reproduces a race condition where multiple threads attempt to
//! build artifacts in the same directory simultaneously.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/8113

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

static BUILD_DIR: &str = "/tmp/sui_build_race_test";

/// Simulates a build operation that creates a directory and writes files
fn build_package_buggy(thread_id: usize, success_count: Arc<AtomicUsize>, error_count: Arc<AtomicUsize>) {
    let build_path = PathBuf::from(BUILD_DIR);

    // BUG: Multiple threads race to create the same directory
    // Check-then-act race condition
    if !build_path.exists() {
        // Race window: Another thread may create the dir between check and create
        match fs::create_dir_all(&build_path) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("[Thread {}] Failed to create dir: {}", thread_id, e);
                error_count.fetch_add(1, Ordering::SeqCst);
                return;
            }
        }
    }

    // BUG: Multiple threads race to write to the same file
    let output_file = build_path.join("output.txt");

    // Simulate some work before writing
    thread::sleep(std::time::Duration::from_micros(100));

    // Race: Multiple threads writing to same file
    match File::create(&output_file) {
        Ok(mut file) => {
            // Write thread ID to detect overwrites
            let content = format!("Built by thread {}\n", thread_id);
            match file.write_all(content.as_bytes()) {
                Ok(_) => {
                    success_count.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) => {
                    eprintln!("[Thread {}] Write error: {}", thread_id, e);
                    error_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        Err(e) => {
            eprintln!("[Thread {}] File create error: {}", thread_id, e);
            error_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    // BUG: Multiple threads race to create subdirectories
    let artifact_dir = build_path.join(format!("artifacts_{}", thread_id));
    if let Err(e) = fs::create_dir(&artifact_dir) {
        // This might fail if another thread's cleanup races with our create
        eprintln!("[Thread {}] Artifact dir error: {}", thread_id, e);
    }
}

/// Fixed version: Each thread uses its own temporary directory
fn build_package_fixed(thread_id: usize, success_count: Arc<AtomicUsize>, _error_count: Arc<AtomicUsize>) {
    // FIX: Each build operation gets its own unique directory
    let build_path = PathBuf::from(format!("{}/build_{}", BUILD_DIR, thread_id));

    // No race: Each thread has its own directory
    fs::create_dir_all(&build_path).expect("Failed to create unique build dir");

    let output_file = build_path.join("output.txt");

    thread::sleep(std::time::Duration::from_micros(100));

    let mut file = File::create(&output_file).expect("Failed to create file");
    let content = format!("Built by thread {}\n", thread_id);
    file.write_all(content.as_bytes()).expect("Failed to write");

    success_count.fetch_add(1, Ordering::SeqCst);

    // Cleanup our own directory
    let _ = fs::remove_dir_all(&build_path);
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #8113: Concurrent Build Directory Race ===\n");

    if use_fixed {
        println!("Running FIXED version (isolated directories)...\n");
    } else {
        println!("Running BUGGY version (shared directory)...\n");
    }

    // Clean up any previous test artifacts
    let _ = fs::remove_dir_all(BUILD_DIR);

    let num_threads = 10;
    let success_count = Arc::new(AtomicUsize::new(0));
    let error_count = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];

    for i in 0..num_threads {
        let success = Arc::clone(&success_count);
        let errors = Arc::clone(&error_count);

        let handle = thread::spawn(move || {
            if use_fixed {
                build_package_fixed(i, success, errors);
            } else {
                build_package_buggy(i, success, errors);
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let successes = success_count.load(Ordering::SeqCst);
    let errors = error_count.load(Ordering::SeqCst);

    println!("\n=== Results ===");
    println!("Successful builds: {}", successes);
    println!("Failed builds: {}", errors);

    // Check final state of shared file (buggy version only)
    if !use_fixed {
        let output_file = PathBuf::from(BUILD_DIR).join("output.txt");
        if output_file.exists() {
            let content = fs::read_to_string(&output_file).unwrap_or_default();
            println!("\nFinal output.txt content:\n{}", content);
            println!("Note: Only one thread's output survived (last writer wins)");
            println!("      {} threads' work was lost!", num_threads - 1);
        }
    }

    // Cleanup
    let _ = fs::remove_dir_all(BUILD_DIR);

    if !use_fixed {
        println!("\n[BUG DEMONSTRATED]");
        println!("Multiple threads raced to write to the same file.");
        println!("All {} threads reported success, but only 1 thread's data persisted.", successes);
        println!("\nRun with --fixed to see the correct behavior.");
    } else {
        println!("\n[FIXED]");
        println!("Each thread used its own directory - no race condition.");
    }
}
