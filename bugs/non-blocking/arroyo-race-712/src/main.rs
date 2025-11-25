//! Arroyo Issue #712: Task Startup Race Condition
//!
//! This reproduces a race condition in Arroyo's pipeline scheduling where:
//! 1. Controller marks pipeline as "running" before operators finish starting
//! 2. If an operator panics during startup, the failure notification is ignored
//! 3. Pipeline appears "healthy" but is actually non-functional
//!
//! Original PR: https://github.com/ArroyoSystems/arroyo/pull/712

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

pub type TaskId = u64;
pub type PipelineId = u64;

/// Pipeline state
#[derive(Clone, Debug, PartialEq)]
pub enum PipelineState {
    Scheduling,
    Running,
    Failed(String),
}

/// Task notification types
#[derive(Clone, Debug)]
pub enum TaskNotification {
    Started(TaskId),
    Failed(TaskId, String),
}

/// Represents an operator task in the pipeline
#[derive(Clone, Debug)]
pub struct Task {
    id: TaskId,
    name: String,
    should_panic: bool, // For testing
}

/// Buggy version - marks pipeline running before operators complete startup
mod buggy {
    use super::*;

    pub struct Controller {
        pipeline_state: Mutex<PipelineState>,
        tasks: Mutex<HashMap<TaskId, Task>>,
        started_tasks: Mutex<Vec<TaskId>>,
    }

    impl Controller {
        pub fn new() -> Self {
            Self {
                pipeline_state: Mutex::new(PipelineState::Scheduling),
                tasks: Mutex::new(HashMap::new()),
                started_tasks: Mutex::new(Vec::new()),
            }
        }

        pub fn add_task(&self, task: Task) {
            self.tasks.lock().unwrap().insert(task.id, task);
        }

        /// BUG: Transitions to Running when TaskStarted received,
        /// but TaskStarted is sent BEFORE on_start completes
        pub fn handle_notification(&self, notification: TaskNotification) {
            let state = self.pipeline_state.lock().unwrap().clone();

            match notification {
                TaskNotification::Started(task_id) => {
                    println!("[BUGGY] Received TaskStarted for task {}", task_id);
                    self.started_tasks.lock().unwrap().push(task_id);

                    // BUG: Check if all tasks "started" and transition to Running
                    let tasks = self.tasks.lock().unwrap();
                    let started = self.started_tasks.lock().unwrap();
                    if started.len() == tasks.len() {
                        println!("[BUGGY] All tasks started, transitioning to Running");
                        *self.pipeline_state.lock().unwrap() = PipelineState::Running;
                    }
                }
                TaskNotification::Failed(task_id, reason) => {
                    println!("[BUGGY] Received TaskFailed for task {}: {}", task_id, reason);

                    // BUG: If we're in Scheduling state, ignore the failure!
                    // This is because TaskFailed during scheduling was treated
                    // as a "RunningMessage" and discarded
                    if state == PipelineState::Scheduling {
                        println!("[BUGGY] Ignoring failure during scheduling phase!");
                        return;
                    }

                    *self.pipeline_state.lock().unwrap() =
                        PipelineState::Failed(reason);
                }
            }
        }

        pub fn get_state(&self) -> PipelineState {
            self.pipeline_state.lock().unwrap().clone()
        }
    }

    pub struct Worker {
        controller: Arc<Controller>,
    }

    impl Worker {
        pub fn new(controller: Arc<Controller>) -> Self {
            Self { controller }
        }

        /// BUG: Sends TaskStarted BEFORE completing on_start
        pub fn start_task(&self, task: Task) {
            let task_id = task.id;
            let should_panic = task.should_panic;
            let name = task.name.clone();

            // BUG: Send TaskStarted BEFORE on_start completes!
            println!("[BUGGY] Task {} sending TaskStarted (before on_start)", task_id);
            self.controller.handle_notification(TaskNotification::Started(task_id));

            // Simulate on_start
            println!("[BUGGY] Task {} executing on_start...", task_id);
            thread::sleep(Duration::from_millis(50));

            if should_panic {
                // Task panics during on_start
                println!("[BUGGY] Task {} ({}) PANICKED during on_start!", task_id, name);
                self.controller.handle_notification(
                    TaskNotification::Failed(task_id, format!("{} panicked", name))
                );
            } else {
                println!("[BUGGY] Task {} on_start completed successfully", task_id);
            }
        }
    }
}

/// Fixed version - sends TaskStarted after on_start completes
mod fixed {
    use super::*;

    pub struct Controller {
        pipeline_state: Mutex<PipelineState>,
        tasks: Mutex<HashMap<TaskId, Task>>,
        started_tasks: Mutex<Vec<TaskId>>,
    }

    impl Controller {
        pub fn new() -> Self {
            Self {
                pipeline_state: Mutex::new(PipelineState::Scheduling),
                tasks: Mutex::new(HashMap::new()),
                started_tasks: Mutex::new(Vec::new()),
            }
        }

        pub fn add_task(&self, task: Task) {
            self.tasks.lock().unwrap().insert(task.id, task);
        }

        /// FIX: Handle TaskFailed during scheduling phase
        pub fn handle_notification(&self, notification: TaskNotification) {
            match notification {
                TaskNotification::Started(task_id) => {
                    println!("[FIXED] Received TaskStarted for task {}", task_id);
                    self.started_tasks.lock().unwrap().push(task_id);

                    let tasks = self.tasks.lock().unwrap();
                    let started = self.started_tasks.lock().unwrap();
                    if started.len() == tasks.len() {
                        println!("[FIXED] All tasks started, transitioning to Running");
                        *self.pipeline_state.lock().unwrap() = PipelineState::Running;
                    }
                }
                TaskNotification::Failed(task_id, reason) => {
                    println!("[FIXED] Received TaskFailed for task {}: {}", task_id, reason);

                    // FIX: Handle failure during scheduling phase!
                    // Trigger rescheduling instead of ignoring
                    let state = self.pipeline_state.lock().unwrap().clone();
                    if state == PipelineState::Scheduling {
                        println!("[FIXED] Failure during scheduling - triggering reschedule");
                    }

                    *self.pipeline_state.lock().unwrap() =
                        PipelineState::Failed(reason);
                }
            }
        }

        pub fn get_state(&self) -> PipelineState {
            self.pipeline_state.lock().unwrap().clone()
        }
    }

    pub struct Worker {
        controller: Arc<Controller>,
    }

    impl Worker {
        pub fn new(controller: Arc<Controller>) -> Self {
            Self { controller }
        }

        /// FIX: Send TaskStarted AFTER on_start completes
        pub fn start_task(&self, task: Task) {
            let task_id = task.id;
            let should_panic = task.should_panic;
            let name = task.name.clone();

            // FIX: Execute on_start FIRST
            println!("[FIXED] Task {} executing on_start...", task_id);
            thread::sleep(Duration::from_millis(50));

            if should_panic {
                println!("[FIXED] Task {} ({}) PANICKED during on_start!", task_id, name);
                self.controller.handle_notification(
                    TaskNotification::Failed(task_id, format!("{} panicked", name))
                );
                return; // Don't send TaskStarted!
            }

            // FIX: Only send TaskStarted after successful on_start
            println!("[FIXED] Task {} on_start completed, sending TaskStarted", task_id);
            self.controller.handle_notification(TaskNotification::Started(task_id));
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Arroyo Issue #712: Task Startup Race Condition ===\n");

    if use_fixed {
        println!("Running FIXED version (TaskStarted after on_start)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (TaskStarted before on_start)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let controller = Arc::new(buggy::Controller::new());

    // Create tasks - task 2 will panic during startup
    let tasks = vec![
        Task { id: 1, name: "source".to_string(), should_panic: false },
        Task { id: 2, name: "transform".to_string(), should_panic: true },
        Task { id: 3, name: "sink".to_string(), should_panic: false },
    ];

    for task in &tasks {
        controller.add_task(task.clone());
    }

    println!("Scenario: Start 3 tasks, task 2 (transform) will panic during on_start\n");

    let worker = Arc::new(buggy::Worker::new(Arc::clone(&controller)));

    // Start all tasks concurrently
    let mut handles = vec![];
    for task in tasks {
        let w = Arc::clone(&worker);
        handles.push(thread::spawn(move || {
            w.start_task(task);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Check final state
    let state = controller.get_state();

    println!("\n=== Results ===");
    println!("Final pipeline state: {:?}", state);

    if state == PipelineState::Running {
        println!("\n[BUG DEMONSTRATED]");
        println!("Pipeline shows as 'Running' but task 2 failed!");
        println!("\nProblem:");
        println!("  - TaskStarted sent BEFORE on_start completes");
        println!("  - All TaskStarted received -> pipeline marked Running");
        println!("  - TaskFailed during scheduling was IGNORED");
        println!("  - Pipeline appears healthy but is broken");
        println!("\nRun with --fixed to see correct behavior.");
    }
}

fn run_fixed_test() {
    let controller = Arc::new(fixed::Controller::new());

    let tasks = vec![
        Task { id: 1, name: "source".to_string(), should_panic: false },
        Task { id: 2, name: "transform".to_string(), should_panic: true },
        Task { id: 3, name: "sink".to_string(), should_panic: false },
    ];

    for task in &tasks {
        controller.add_task(task.clone());
    }

    println!("Scenario: Start 3 tasks, task 2 (transform) will panic during on_start\n");

    let worker = Arc::new(fixed::Worker::new(Arc::clone(&controller)));

    let mut handles = vec![];
    for task in tasks {
        let w = Arc::clone(&worker);
        handles.push(thread::spawn(move || {
            w.start_task(task);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let state = controller.get_state();

    println!("\n=== Results ===");
    println!("Final pipeline state: {:?}", state);

    if let PipelineState::Failed(reason) = state {
        println!("\n[FIXED]");
        println!("Pipeline correctly shows as Failed: {}", reason);
        println!("\nFix:");
        println!("  - TaskStarted sent AFTER on_start completes");
        println!("  - TaskFailed during scheduling triggers reschedule");
        println!("  - Failed tasks are properly detected");
    }
}
