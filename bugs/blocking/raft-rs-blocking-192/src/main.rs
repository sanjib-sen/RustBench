//! raft-rs Issue #192: Joint Consensus Blocking
//!
//! This reproduces a blocking bug in Raft joint consensus where the cluster
//! can get stuck when transitioning between configurations. During a configuration
//! change, both the old (C_old) and new (C_new) quorums must agree. If either
//! cannot achieve quorum (due to network partition or failures), the cluster
//! blocks indefinitely.
//!
//! Original Issue: https://github.com/tikv/raft-rs/issues/192

use std::collections::HashSet;
use std::env;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

pub type NodeId = u64;
pub type Term = u64;
pub type LogIndex = u64;

/// Represents a Raft configuration (set of voter nodes)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    voters: HashSet<NodeId>,
}

impl Configuration {
    fn new(voters: &[NodeId]) -> Self {
        Self {
            voters: voters.iter().cloned().collect(),
        }
    }

    fn quorum_size(&self) -> usize {
        self.voters.len() / 2 + 1
    }

    fn has_quorum(&self, votes: &HashSet<NodeId>) -> bool {
        let count = self.voters.intersection(votes).count();
        count >= self.quorum_size()
    }
}

/// Joint configuration state during transition
#[derive(Clone, Debug)]
pub struct JointConfiguration {
    c_old: Configuration,
    c_new: Configuration,
}

impl JointConfiguration {
    fn new(c_old: Configuration, c_new: Configuration) -> Self {
        Self { c_old, c_new }
    }

    /// Joint consensus requires both configurations to have quorum
    fn has_joint_quorum(&self, votes: &HashSet<NodeId>) -> bool {
        self.c_old.has_quorum(votes) && self.c_new.has_quorum(votes)
    }
}

/// Represents a log entry for configuration change
#[derive(Clone, Debug)]
pub enum LogEntry {
    ConfigChange(JointConfiguration),
    Normal(Vec<u8>),
}

/// Replication status from a node
#[derive(Debug)]
pub struct ReplicationStatus {
    node: NodeId,
    success: bool,
    match_index: LogIndex,
}

/// Buggy version - blocks indefinitely when joint quorum cannot be achieved
mod buggy {
    use super::*;

    pub struct RaftNode {
        id: NodeId,
        current_config: Mutex<Option<JointConfiguration>>,
        committed_index: Mutex<LogIndex>,
        replication_responses: Mutex<HashSet<NodeId>>,
        blocked: Mutex<bool>,
        block_cvar: Condvar,
    }

    impl RaftNode {
        pub fn new(id: NodeId) -> Self {
            Self {
                id,
                current_config: Mutex::new(None),
                committed_index: Mutex::new(0),
                replication_responses: Mutex::new(HashSet::new()),
                blocked: Mutex::new(false),
                block_cvar: Condvar::new(),
            }
        }

        /// BUG: Start a configuration change that may block forever
        pub fn begin_config_change(&self, c_old: Configuration, c_new: Configuration) {
            println!("[BUGGY] Node {} starting config change", self.id);
            println!("[BUGGY] C_old: {:?}", c_old.voters);
            println!("[BUGGY] C_new: {:?}", c_new.voters);

            let joint = JointConfiguration::new(c_old, c_new);
            *self.current_config.lock().unwrap() = Some(joint.clone());

            // Add self to replication responses
            self.replication_responses.lock().unwrap().insert(self.id);

            println!("[BUGGY] Entered joint consensus state, waiting for quorum...");
        }

        /// Receive replication response from a follower
        pub fn receive_replication_response(&self, status: ReplicationStatus) {
            println!("[BUGGY] Received response from node {}: success={}",
                     status.node, status.success);

            if status.success {
                self.replication_responses.lock().unwrap().insert(status.node);
            }

            self.check_commit_progress();
        }

        /// BUG: Blocks indefinitely if joint quorum cannot be achieved
        fn check_commit_progress(&self) {
            let config = self.current_config.lock().unwrap();
            let responses = self.replication_responses.lock().unwrap();

            if let Some(ref joint) = *config {
                let has_old_quorum = joint.c_old.has_quorum(&responses);
                let has_new_quorum = joint.c_new.has_quorum(&responses);

                println!("[BUGGY] Checking progress: C_old quorum={}, C_new quorum={}",
                         has_old_quorum, has_new_quorum);

                if joint.has_joint_quorum(&responses) {
                    println!("[BUGGY] Joint quorum achieved! Committing config change.");
                    *self.committed_index.lock().unwrap() = 1;
                } else {
                    // BUG: No timeout or rollback mechanism!
                    // If we have C_old quorum but not C_new (or vice versa),
                    // we're stuck forever waiting
                    if has_old_quorum && !has_new_quorum {
                        println!("[BUGGY] WARNING: Have C_old quorum but NOT C_new quorum!");
                        println!("[BUGGY] BLOCKED: Cannot proceed without both quorums!");
                        *self.blocked.lock().unwrap() = true;
                    }
                }
            }
        }

        /// Wait for commit with timeout (to demonstrate blocking)
        pub fn wait_for_commit(&self, timeout: Duration) -> bool {
            let start = std::time::Instant::now();
            loop {
                let committed = *self.committed_index.lock().unwrap();
                if committed > 0 {
                    return true;
                }

                if *self.blocked.lock().unwrap() {
                    return false;
                }

                if start.elapsed() >= timeout {
                    return false;
                }

                thread::sleep(Duration::from_millis(50));
            }
        }

        pub fn is_blocked(&self) -> bool {
            *self.blocked.lock().unwrap()
        }
    }
}

/// Fixed version - implements timeout and rollback
mod fixed {
    use super::*;

    pub struct RaftNode {
        id: NodeId,
        current_config: Mutex<Option<JointConfiguration>>,
        original_config: Mutex<Option<Configuration>>,
        committed_index: Mutex<LogIndex>,
        replication_responses: Mutex<HashSet<NodeId>>,
        config_change_start: Mutex<Option<std::time::Instant>>,
        rolled_back: Mutex<bool>,
    }

    impl RaftNode {
        pub fn new(id: NodeId) -> Self {
            Self {
                id,
                current_config: Mutex::new(None),
                original_config: Mutex::new(None),
                committed_index: Mutex::new(0),
                replication_responses: Mutex::new(HashSet::new()),
                config_change_start: Mutex::new(None),
                rolled_back: Mutex::new(false),
            }
        }

        pub fn begin_config_change(&self, c_old: Configuration, c_new: Configuration) {
            println!("[FIXED] Node {} starting config change", self.id);
            println!("[FIXED] C_old: {:?}", c_old.voters);
            println!("[FIXED] C_new: {:?}", c_new.voters);

            // FIX: Save original config for potential rollback
            *self.original_config.lock().unwrap() = Some(c_old.clone());

            let joint = JointConfiguration::new(c_old, c_new);
            *self.current_config.lock().unwrap() = Some(joint.clone());

            // FIX: Record start time for timeout
            *self.config_change_start.lock().unwrap() = Some(std::time::Instant::now());

            self.replication_responses.lock().unwrap().insert(self.id);

            println!("[FIXED] Entered joint consensus state, waiting for quorum...");
        }

        pub fn receive_replication_response(&self, status: ReplicationStatus) {
            println!("[FIXED] Received response from node {}: success={}",
                     status.node, status.success);

            if status.success {
                self.replication_responses.lock().unwrap().insert(status.node);
            }

            self.check_commit_progress();
        }

        /// FIX: Check for timeout and rollback if needed
        fn check_commit_progress(&self) {
            let config = self.current_config.lock().unwrap().clone();
            let responses = self.replication_responses.lock().unwrap().clone();

            if let Some(ref joint) = config {
                let has_old_quorum = joint.c_old.has_quorum(&responses);
                let has_new_quorum = joint.c_new.has_quorum(&responses);

                println!("[FIXED] Checking progress: C_old quorum={}, C_new quorum={}",
                         has_old_quorum, has_new_quorum);

                if joint.has_joint_quorum(&responses) {
                    println!("[FIXED] Joint quorum achieved! Committing config change.");
                    *self.committed_index.lock().unwrap() = 1;
                } else if has_old_quorum && !has_new_quorum {
                    // FIX: Check if we should rollback
                    self.maybe_rollback();
                }
            }
        }

        /// FIX: Rollback to original config after timeout
        fn maybe_rollback(&self) {
            let start_time = self.config_change_start.lock().unwrap().clone();
            let config_change_timeout = Duration::from_millis(500);

            if let Some(start) = start_time {
                if start.elapsed() >= config_change_timeout {
                    println!("[FIXED] Config change timeout! Rolling back to original config.");

                    // Rollback to C_old
                    if let Some(original) = self.original_config.lock().unwrap().take() {
                        println!("[FIXED] Rolled back to config: {:?}", original.voters);
                        *self.current_config.lock().unwrap() = None;
                        *self.rolled_back.lock().unwrap() = true;
                        *self.committed_index.lock().unwrap() = 1; // Mark as resolved
                    }
                }
            }
        }

        pub fn wait_for_commit(&self, timeout: Duration) -> bool {
            let start = std::time::Instant::now();
            loop {
                // Trigger progress check with timeout handling
                self.check_commit_progress();

                let committed = *self.committed_index.lock().unwrap();
                if committed > 0 {
                    return true;
                }

                if start.elapsed() >= timeout {
                    return false;
                }

                thread::sleep(Duration::from_millis(50));
            }
        }

        pub fn was_rolled_back(&self) -> bool {
            *self.rolled_back.lock().unwrap()
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== raft-rs Issue #192: Joint Consensus Blocking ===\n");

    if use_fixed {
        println!("Running FIXED version (timeout and rollback)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (blocks indefinitely)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    println!("Scenario: Config change from [A,B,C] to [D,E]");
    println!("Problem: New config nodes D,E are unreachable\n");

    let leader = Arc::new(buggy::RaftNode::new(1)); // Node A is leader

    // Old config: A(1), B(2), C(3)
    // New config: D(4), E(5) - both unreachable
    let c_old = Configuration::new(&[1, 2, 3]);
    let c_new = Configuration::new(&[4, 5]);

    // Start config change
    leader.begin_config_change(c_old, c_new);

    // Simulate responses from old config nodes (A, B, C respond)
    let leader_clone = Arc::clone(&leader);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        leader_clone.receive_replication_response(ReplicationStatus {
            node: 2, // B
            success: true,
            match_index: 1,
        });
    });

    let leader_clone = Arc::clone(&leader);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        leader_clone.receive_replication_response(ReplicationStatus {
            node: 3, // C
            success: true,
            match_index: 1,
        });
    });

    // New config nodes D(4), E(5) never respond - they're unreachable

    // Wait for commit with timeout
    let timeout = Duration::from_secs(2);
    let committed = leader.wait_for_commit(timeout);

    println!("\n=== Results ===");
    if !committed && leader.is_blocked() {
        println!("[BUG DEMONSTRATED]");
        println!("Cluster is BLOCKED!");
        println!("\nProblem:");
        println!("  - Have quorum from C_old (A, B, C responded)");
        println!("  - No quorum from C_new (D, E unreachable)");
        println!("  - Joint consensus requires BOTH quorums");
        println!("  - No timeout or rollback mechanism!");
        println!("  - Cluster stuck indefinitely");
        println!("\nRun with --fixed to see timeout/rollback.");
    } else if committed {
        println!("Config change committed (unexpected)");
    } else {
        println!("Timed out waiting for commit");
    }
}

fn run_fixed_test() {
    println!("Scenario: Config change from [A,B,C] to [D,E]");
    println!("Problem: New config nodes D,E are unreachable");
    println!("Fix: Rollback to C_old after timeout\n");

    let leader = Arc::new(fixed::RaftNode::new(1));

    let c_old = Configuration::new(&[1, 2, 3]);
    let c_new = Configuration::new(&[4, 5]);

    leader.begin_config_change(c_old, c_new);

    let leader_clone = Arc::clone(&leader);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        leader_clone.receive_replication_response(ReplicationStatus {
            node: 2,
            success: true,
            match_index: 1,
        });
    });

    let leader_clone = Arc::clone(&leader);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        leader_clone.receive_replication_response(ReplicationStatus {
            node: 3,
            success: true,
            match_index: 1,
        });
    });

    // Wait for commit (should succeed via rollback)
    let timeout = Duration::from_secs(2);
    let committed = leader.wait_for_commit(timeout);

    println!("\n=== Results ===");
    if committed && leader.was_rolled_back() {
        println!("[FIXED]");
        println!("Config change rolled back successfully!");
        println!("\nFix: Implemented timeout and rollback");
        println!("  - After timeout, rollback to original config");
        println!("  - Cluster remains operational with C_old");
        println!("  - Admin can retry config change later");
    } else if committed {
        println!("Config change committed normally");
    } else {
        println!("Unexpected: timed out");
    }
}
