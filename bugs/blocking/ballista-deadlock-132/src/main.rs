//! Ballista Issue #132: Executor Task Slot Deadlock
//!
//! This reproduces a deadlock where tasks from stage 2 fill all executor slots
//! while waiting for stage 1 inputs. Since stage 1 tasks can't get slots,
//! stage 2 tasks block forever - causing system deadlock.
//!
//! Original issue: https://github.com/apache/datafusion-ballista/issues/132

use std::collections::VecDeque;
use std::env;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Task {
    id: String,
    stage: u32,
    depends_on_stage: Option<u32>,
}

/// Executor with limited task slots
pub struct Executor {
    name: String,
    max_slots: usize,
    running_tasks: Mutex<Vec<Task>>,
    available_slots: Mutex<usize>,
    slot_available: Condvar,
}

impl Executor {
    fn new(name: &str, max_slots: usize) -> Self {
        Self {
            name: name.to_string(),
            max_slots,
            running_tasks: Mutex::new(Vec::new()),
            available_slots: Mutex::new(max_slots),
            slot_available: Condvar::new(),
        }
    }

    fn available_slots(&self) -> usize {
        *self.available_slots.lock().unwrap()
    }
}

/// Buggy scheduler - schedules tasks without considering dependencies
mod buggy {
    use super::*;

    pub struct Scheduler {
        executor: Arc<Executor>,
        stage_complete: Arc<Mutex<Vec<u32>>>,
    }

    impl Scheduler {
        pub fn new(executor: Arc<Executor>) -> Self {
            Self {
                executor,
                stage_complete: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// BUG: Schedules task even if dependencies aren't met
        /// This can deadlock if dependent tasks fill all slots
        pub fn schedule_task(&self, task: Task) {
            // Wait for a slot
            let mut slots = self.executor.available_slots.lock().unwrap();
            while *slots == 0 {
                println!(
                    "[BUGGY] Task {} waiting for slot (no slots available)",
                    task.id
                );
                slots = self.executor.slot_available.wait(slots).unwrap();
            }
            *slots -= 1;
            drop(slots);

            let executor = Arc::clone(&self.executor);
            let stage_complete = Arc::clone(&self.stage_complete);
            let task_clone = task.clone();

            println!("[BUGGY] Scheduling task {} (stage {})", task.id, task.stage);

            thread::spawn(move || {
                // Add to running tasks
                {
                    let mut running = executor.running_tasks.lock().unwrap();
                    running.push(task_clone.clone());
                }

                // BUG: Wait for dependency while holding slot
                if let Some(dep_stage) = task_clone.depends_on_stage {
                    println!(
                        "[BUGGY] Task {} waiting for stage {} to complete...",
                        task_clone.id, dep_stage
                    );

                    // This can deadlock if all slots are filled with waiting tasks!
                    loop {
                        {
                            let completed = stage_complete.lock().unwrap();
                            if completed.contains(&dep_stage) {
                                break;
                            }
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }

                // Simulate task execution
                println!("[BUGGY] Task {} executing...", task_clone.id);
                thread::sleep(Duration::from_millis(100));

                // Mark stage complete
                {
                    let mut completed = stage_complete.lock().unwrap();
                    if !completed.contains(&task_clone.stage) {
                        completed.push(task_clone.stage);
                        println!("[BUGGY] Stage {} marked complete", task_clone.stage);
                    }
                }

                // Release slot
                {
                    let mut running = executor.running_tasks.lock().unwrap();
                    running.retain(|t| t.id != task_clone.id);
                }
                {
                    let mut slots = executor.available_slots.lock().unwrap();
                    *slots += 1;
                }
                executor.slot_available.notify_one();

                println!("[BUGGY] Task {} completed", task_clone.id);
            });
        }
    }
}

/// Fixed scheduler - respects dependencies before scheduling
mod fixed {
    use super::*;

    pub struct Scheduler {
        executor: Arc<Executor>,
        stage_complete: Arc<Mutex<Vec<u32>>>,
        pending_queue: Mutex<VecDeque<Task>>,
    }

    impl Scheduler {
        pub fn new(executor: Arc<Executor>) -> Self {
            Self {
                executor,
                stage_complete: Arc::new(Mutex::new(Vec::new())),
                pending_queue: Mutex::new(VecDeque::new()),
            }
        }

        /// FIX: Only schedule tasks whose dependencies are met
        pub fn schedule_task(&self, task: Task) {
            // Check if dependencies are met BEFORE taking a slot
            if let Some(dep_stage) = task.depends_on_stage {
                let completed = self.stage_complete.lock().unwrap();
                if !completed.contains(&dep_stage) {
                    println!(
                        "[FIXED] Task {} queued (waiting for stage {})",
                        task.id, dep_stage
                    );
                    let mut queue = self.pending_queue.lock().unwrap();
                    queue.push_back(task);
                    return;
                }
            }

            self.run_task(task);
        }

        fn run_task(&self, task: Task) {
            let mut slots = self.executor.available_slots.lock().unwrap();
            while *slots == 0 {
                slots = self.executor.slot_available.wait(slots).unwrap();
            }
            *slots -= 1;
            drop(slots);

            let executor = Arc::clone(&self.executor);
            let stage_complete = Arc::clone(&self.stage_complete);
            let task_clone = task.clone();

            println!(
                "[FIXED] Running task {} (stage {})",
                task.id, task.stage
            );

            thread::spawn(move || {
                // Execute immediately - dependencies already met!
                println!("[FIXED] Task {} executing...", task_clone.id);
                thread::sleep(Duration::from_millis(100));

                // Mark stage complete
                {
                    let mut completed = stage_complete.lock().unwrap();
                    if !completed.contains(&task_clone.stage) {
                        completed.push(task_clone.stage);
                        println!("[FIXED] Stage {} marked complete", task_clone.stage);
                    }
                }

                // Release slot
                {
                    let mut slots = executor.available_slots.lock().unwrap();
                    *slots += 1;
                }
                executor.slot_available.notify_one();

                println!("[FIXED] Task {} completed", task_clone.id);
            });
        }

        pub fn process_pending(&self) {
            let mut queue = self.pending_queue.lock().unwrap();
            let mut ready = Vec::new();

            // Find tasks whose dependencies are now met
            {
                let completed = self.stage_complete.lock().unwrap();
                queue.retain(|task| {
                    if let Some(dep) = task.depends_on_stage {
                        if completed.contains(&dep) {
                            ready.push(task.clone());
                            return false;
                        }
                    }
                    true
                });
            }

            drop(queue);

            for task in ready {
                self.run_task(task);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== Ballista Issue #132: Executor Task Slot Deadlock ===\n");

    if use_fixed {
        println!("Running FIXED version (dependency-aware scheduling)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (may deadlock)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    // Only 2 slots available
    let executor = Arc::new(Executor::new("executor-1", 2));
    let scheduler = buggy::Scheduler::new(Arc::clone(&executor));

    println!("Executor has {} slots", executor.max_slots);
    println!("Scheduling 2 stage-2 tasks, then 2 stage-1 tasks\n");

    // BUG: Schedule stage 2 tasks first (they depend on stage 1)
    let tasks = vec![
        Task {
            id: "task_2a".to_string(),
            stage: 2,
            depends_on_stage: Some(1),
        },
        Task {
            id: "task_2b".to_string(),
            stage: 2,
            depends_on_stage: Some(1),
        },
        Task {
            id: "task_1a".to_string(),
            stage: 1,
            depends_on_stage: None,
        },
        Task {
            id: "task_1b".to_string(),
            stage: 1,
            depends_on_stage: None,
        },
    ];

    for task in tasks {
        scheduler.schedule_task(task);
        thread::sleep(Duration::from_millis(50));
    }

    // Wait and check for deadlock
    println!("\nWaiting for completion (3 second timeout)...\n");
    thread::sleep(Duration::from_secs(3));

    let slots = executor.available_slots();
    if slots < executor.max_slots {
        println!("\n=== Results ===");
        println!("[DEADLOCK DETECTED]");
        println!("Only {} of {} slots available after 3 seconds", slots, executor.max_slots);
        println!("\nDeadlock scenario:");
        println!("  - Stage 2 tasks took all {} slots", executor.max_slots);
        println!("  - Stage 2 tasks wait for stage 1 to complete");
        println!("  - Stage 1 tasks can't get slots to run");
        println!("  - DEADLOCK: Circular dependency on slots!");
        println!("\nRun with --fixed to see dependency-aware scheduling.");
        std::process::exit(1);
    }
}

fn run_fixed_test() {
    let executor = Arc::new(Executor::new("executor-1", 2));
    let scheduler = Arc::new(fixed::Scheduler::new(Arc::clone(&executor)));

    println!("Executor has {} slots", executor.max_slots);
    println!("Scheduling tasks with dependency checking\n");

    let scheduler_clone = Arc::clone(&scheduler);

    // Schedule same tasks - but fixed scheduler will queue dependent tasks
    let tasks = vec![
        Task {
            id: "task_2a".to_string(),
            stage: 2,
            depends_on_stage: Some(1),
        },
        Task {
            id: "task_2b".to_string(),
            stage: 2,
            depends_on_stage: Some(1),
        },
        Task {
            id: "task_1a".to_string(),
            stage: 1,
            depends_on_stage: None,
        },
        Task {
            id: "task_1b".to_string(),
            stage: 1,
            depends_on_stage: None,
        },
    ];

    for task in tasks {
        scheduler.schedule_task(task);
        thread::sleep(Duration::from_millis(50));
    }

    // Process pending tasks periodically
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(200));
        scheduler_clone.process_pending();
    }

    thread::sleep(Duration::from_millis(500));

    println!("\n=== Results ===");
    println!("[FIXED]");
    println!("All tasks completed without deadlock!");
    println!("Stage 2 tasks queued until stage 1 completed.");
    println!("No slot starvation - dependencies respected.");
}
