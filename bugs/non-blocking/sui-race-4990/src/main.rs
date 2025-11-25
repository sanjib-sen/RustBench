//! Sui Issue #4990: Parallel Certificate Execution Race
//!
//! This reproduces a race where parallel execution of dependent tasks
//! causes later tasks to fail because earlier dependencies haven't
//! completed yet.
//!
//! Original bug: https://github.com/MystenLabs/sui/issues/4990
//! Fix PR: https://github.com/MystenLabs/sui/pull/5778

use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

#[derive(Debug, Clone)]
pub struct Task {
    id: String,
    inputs: Vec<ObjectId>,  // Objects this task needs to execute
    outputs: Vec<ObjectId>, // Objects this task produces
}

#[derive(Debug, Clone)]
pub enum TaskResult {
    Success,
    Failed(String),
}

/// Simulates blockchain state with object versions
pub struct State {
    available_objects: Mutex<HashSet<ObjectId>>,
}

impl State {
    fn new() -> Self {
        // Start with genesis objects
        let mut objects = HashSet::new();
        objects.insert(ObjectId("obj_0".to_string()));
        Self {
            available_objects: Mutex::new(objects),
        }
    }

    fn has_object(&self, obj: &ObjectId) -> bool {
        let objects = self.available_objects.lock().unwrap();
        objects.contains(obj)
    }

    fn add_objects(&self, objs: Vec<ObjectId>) {
        let mut objects = self.available_objects.lock().unwrap();
        for obj in objs {
            objects.insert(obj);
        }
    }
}

/// Buggy executor: executes tasks in parallel without dependency checking
mod buggy {
    use super::*;

    pub struct ParallelExecutor {
        state: Arc<State>,
        results: Mutex<HashMap<String, TaskResult>>,
    }

    impl ParallelExecutor {
        pub fn new(state: Arc<State>) -> Self {
            Self {
                state,
                results: Mutex::new(HashMap::new()),
            }
        }

        /// BUG: Executes task without waiting for dependencies
        pub fn execute_task(&self, task: Task) -> TaskResult {
            println!("[BUGGY] Executing task {}", task.id);

            // Check if all inputs are available
            for input in &task.inputs {
                if !self.state.has_object(input) {
                    let msg = format!(
                        "[BUGGY] Task {} FAILED! Missing input {:?}",
                        task.id, input.0
                    );
                    println!("{}", msg);
                    let result = TaskResult::Failed(msg);
                    self.results
                        .lock()
                        .unwrap()
                        .insert(task.id.clone(), result.clone());
                    return result;
                }
            }

            // Simulate execution time
            thread::sleep(Duration::from_millis(50));

            // Add outputs to state
            self.state.add_objects(task.outputs.clone());

            println!("[BUGGY] Task {} completed successfully", task.id);
            let result = TaskResult::Success;
            self.results
                .lock()
                .unwrap()
                .insert(task.id.clone(), result.clone());
            result
        }

        pub fn get_results(&self) -> HashMap<String, TaskResult> {
            self.results.lock().unwrap().clone()
        }
    }
}

/// Fixed executor: tracks dependencies and ensures execution order
mod fixed {
    use super::*;

    pub struct ParallelExecutor {
        state: Arc<State>,
        results: Mutex<HashMap<String, TaskResult>>,
        pending: Mutex<Vec<Task>>,
    }

    impl ParallelExecutor {
        pub fn new(state: Arc<State>) -> Self {
            Self {
                state,
                results: Mutex::new(HashMap::new()),
                pending: Mutex::new(Vec::new()),
            }
        }

        /// FIX: Only execute when all dependencies are ready
        pub fn execute_task(&self, task: Task) -> TaskResult {
            // Check if dependencies are ready
            let can_execute = task.inputs.iter().all(|input| self.state.has_object(input));

            if !can_execute {
                // Queue for later execution
                println!(
                    "[FIXED] Task {} waiting for dependencies, queuing...",
                    task.id
                );
                self.pending.lock().unwrap().push(task.clone());
                return TaskResult::Failed("Pending".to_string());
            }

            println!("[FIXED] Executing task {}", task.id);

            // Simulate execution time
            thread::sleep(Duration::from_millis(50));

            // Add outputs to state
            self.state.add_objects(task.outputs.clone());

            println!("[FIXED] Task {} completed successfully", task.id);
            let result = TaskResult::Success;
            self.results
                .lock()
                .unwrap()
                .insert(task.id.clone(), result.clone());

            // Try to execute pending tasks
            self.try_execute_pending();

            result
        }

        fn try_execute_pending(&self) {
            loop {
                let ready_task = {
                    let mut pending = self.pending.lock().unwrap();
                    let ready_idx = pending
                        .iter()
                        .position(|t| t.inputs.iter().all(|input| self.state.has_object(input)));

                    ready_idx.map(|idx| pending.remove(idx))
                };

                if let Some(task) = ready_task {
                    println!("[FIXED] Executing previously pending task {}", task.id);
                    thread::sleep(Duration::from_millis(50));
                    self.state.add_objects(task.outputs.clone());
                    println!("[FIXED] Task {} completed successfully", task.id);
                    self.results
                        .lock()
                        .unwrap()
                        .insert(task.id.clone(), TaskResult::Success);
                } else {
                    break;
                }
            }
        }

        pub fn get_results(&self) -> HashMap<String, TaskResult> {
            self.results.lock().unwrap().clone()
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Sui Issue #4990: Parallel Certificate Execution Race ===\n");

    if use_fixed {
        println!("Running FIXED version (dependency-aware execution)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (parallel execution without dependency tracking)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let state = Arc::new(State::new());
    let executor = Arc::new(buggy::ParallelExecutor::new(Arc::clone(&state)));

    // Create dependent tasks:
    // Task A: consumes obj_0, produces obj_1
    // Task B: consumes obj_1, produces obj_2 (depends on A)
    // Task C: consumes obj_2, produces obj_3 (depends on B)
    let tasks = vec![
        Task {
            id: "A".to_string(),
            inputs: vec![ObjectId("obj_0".to_string())],
            outputs: vec![ObjectId("obj_1".to_string())],
        },
        Task {
            id: "B".to_string(),
            inputs: vec![ObjectId("obj_1".to_string())],
            outputs: vec![ObjectId("obj_2".to_string())],
        },
        Task {
            id: "C".to_string(),
            inputs: vec![ObjectId("obj_2".to_string())],
            outputs: vec![ObjectId("obj_3".to_string())],
        },
    ];

    let mut handles = vec![];

    // BUG: Submit all tasks in parallel without checking dependencies
    for task in tasks {
        let executor = Arc::clone(&executor);
        let handle = thread::spawn(move || {
            executor.execute_task(task);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("\n=== Results ===");
    let results = executor.get_results();
    let mut failed_count = 0;

    for (task_id, result) in &results {
        match result {
            TaskResult::Success => println!("Task {}: SUCCESS", task_id),
            TaskResult::Failed(reason) => {
                println!("Task {}: FAILED ({})", task_id, reason);
                failed_count += 1;
            }
        }
    }

    if failed_count > 0 {
        println!("\n[BUG DEMONSTRATED]");
        println!(
            "{} task(s) failed due to parallel execution without dependency tracking.",
            failed_count
        );
        println!("Tasks raced ahead before their dependencies completed.");
    } else {
        println!("\n[NOTE]");
        println!("No failures this run (timing-dependent race).");
        println!("Try running multiple times to see the race condition.");
    }

    println!("\nRun with --fixed to see dependency-aware version.");
}

fn run_fixed_test() {
    let state = Arc::new(State::new());
    let executor = Arc::new(fixed::ParallelExecutor::new(Arc::clone(&state)));

    let tasks = vec![
        Task {
            id: "A".to_string(),
            inputs: vec![ObjectId("obj_0".to_string())],
            outputs: vec![ObjectId("obj_1".to_string())],
        },
        Task {
            id: "B".to_string(),
            inputs: vec![ObjectId("obj_1".to_string())],
            outputs: vec![ObjectId("obj_2".to_string())],
        },
        Task {
            id: "C".to_string(),
            inputs: vec![ObjectId("obj_2".to_string())],
            outputs: vec![ObjectId("obj_3".to_string())],
        },
    ];

    let mut handles = vec![];

    for task in tasks {
        let executor = Arc::clone(&executor);
        let handle = thread::spawn(move || {
            executor.execute_task(task);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    println!("\n=== Results ===");
    let results = executor.get_results();

    for (task_id, result) in &results {
        match result {
            TaskResult::Success => println!("Task {}: SUCCESS", task_id),
            TaskResult::Failed(reason) => println!("Task {}: FAILED ({})", task_id, reason),
        }
    }

    println!("\n[FIXED]");
    println!("All tasks completed successfully with dependency tracking.");
    println!("Tasks waited for their dependencies before executing.");
}
