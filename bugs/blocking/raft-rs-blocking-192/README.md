# raft-rs Issue #192: Joint Consensus Blocking

## Bug Information
- **Source**: raft-rs (TiKV)
- **Issue**: https://github.com/tikv/raft-rs/issues/192
- **Type**: Blocking bug (Deadlock / Livelock)
- **Category**: Missing timeout/rollback in consensus protocol

## Root Cause

During a Raft configuration change using joint consensus, the cluster can get stuck indefinitely when transitioning between configurations. Joint consensus requires quorum from both the old configuration (C_old) and new configuration (C_new) to proceed. If either configuration cannot achieve quorum (due to network partitions or node failures), the cluster blocks forever with no recovery mechanism.

**Pattern**: Protocol-level blocking without timeout/rollback

## Bug Pattern

```
Joint Consensus Flow:
--------------------
1. Leader proposes config change: C_old -> C_new
2. Enter joint state: need quorum from BOTH C_old AND C_new
3. Replicate to all nodes in C_old ∪ C_new

Blocking Scenario:
-----------------
C_old = [A, B, C]  (quorum = 2)
C_new = [D, E]     (quorum = 2)

Replication Responses:
  - A (leader): YES
  - B: YES
  - C: YES
  - D: NO RESPONSE (unreachable)
  - E: NO RESPONSE (unreachable)

Result:
  - C_old quorum: 3/2 = YES ✓
  - C_new quorum: 0/2 = NO ✗
  - Joint quorum: NO

BLOCKED FOREVER!
  - Cannot commit without both quorums
  - No timeout to rollback
  - Cluster is stuck
```

## Reproduction

### Buggy Version
```bash
cargo run
```

**Expected Output**:
```
=== raft-rs Issue #192: Joint Consensus Blocking ===

Running BUGGY version (blocks indefinitely)...

Scenario: Config change from [A,B,C] to [D,E]
Problem: New config nodes D,E are unreachable

[BUGGY] Node 1 starting config change
[BUGGY] C_old: {1, 2, 3}
[BUGGY] C_new: {4, 5}
[BUGGY] Entered joint consensus state, waiting for quorum...
[BUGGY] Received response from node 2: success=true
[BUGGY] Checking progress: C_old quorum=true, C_new quorum=false
[BUGGY] WARNING: Have C_old quorum but NOT C_new quorum!
[BUGGY] BLOCKED: Cannot proceed without both quorums!

=== Results ===
[BUG DEMONSTRATED]
Cluster is BLOCKED!

Problem:
  - Have quorum from C_old (A, B, C responded)
  - No quorum from C_new (D, E unreachable)
  - Joint consensus requires BOTH quorums
  - No timeout or rollback mechanism!
  - Cluster stuck indefinitely
```

### Fixed Version
```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== raft-rs Issue #192: Joint Consensus Blocking ===

Running FIXED version (timeout and rollback)...

Scenario: Config change from [A,B,C] to [D,E]
Problem: New config nodes D,E are unreachable
Fix: Rollback to C_old after timeout

[FIXED] Node 1 starting config change
[FIXED] C_old: {1, 2, 3}
[FIXED] C_new: {4, 5}
[FIXED] Entered joint consensus state, waiting for quorum...
[FIXED] Received response from node 2: success=true
[FIXED] Config change timeout! Rolling back to original config.
[FIXED] Rolled back to config: {1, 2, 3}

=== Results ===
[FIXED]
Config change rolled back successfully!

Fix: Implemented timeout and rollback
  - After timeout, rollback to original config
  - Cluster remains operational with C_old
  - Admin can retry config change later
```

## Fix Strategy

### BUGGY: No timeout or rollback
```rust
fn check_commit_progress(&self) {
    if joint.has_joint_quorum(&responses) {
        commit();
    } else if has_old_quorum && !has_new_quorum {
        // BUG: Just wait forever!
        // No timeout, no rollback
        println!("BLOCKED!");
    }
}
```

### FIXED: Timeout and rollback
```rust
fn check_commit_progress(&self) {
    if joint.has_joint_quorum(&responses) {
        commit();
    } else if has_old_quorum && !has_new_quorum {
        // FIX: Check timeout and rollback
        if elapsed >= config_change_timeout {
            rollback_to_original_config();
        }
    }
}
```

## Distributed System Relevance

This bug is critical for:
- **Consensus systems**: Raft, Paxos configuration changes
- **Distributed databases**: TiKV, CockroachDB membership changes
- **Kubernetes**: etcd cluster reconfiguration
- **Service discovery**: Consul cluster operations

**Real-world impact**:
- Cluster becomes unresponsive during membership changes
- Network partitions can cause permanent blocking
- Requires manual intervention to recover
- Diego Ongaro (Raft author) noted LogCabin implements rollback

## Tool Detection

- **Model checking**: Could detect blocking states in protocol
- **Simulation**: Network partition scenarios would expose issue
- **Testing**: Chaos engineering with node failures
- **Formal verification**: TLA+ spec would catch missing liveness

## Notes

- This is a **protocol-level** bug, not just implementation
- Raft paper doesn't explicitly specify rollback behavior
- Joint consensus is complex: overlapping vs non-overlapping configs differ
- Fix requires careful balance between safety and liveness
- Similar issues exist in other consensus implementations
