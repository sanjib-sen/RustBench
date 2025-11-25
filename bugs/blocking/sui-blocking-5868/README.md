# Sui PR #5868: Batch Notifier Missing Notification on Failure

## Bug Information
- **Source**: Sui Blockchain (MystenLabs)
- **PR**: https://github.com/MystenLabs/sui/pull/5868
- **Type**: Blocking bug (Missing notification / Lost wakeup)
- **Category**: Notification gap on error path

## Root Cause

When a certificate commit operation fails after a sequence number has been assigned, the batch notifier is not informed. This creates a gap in the sequence where dependent operations waiting for that sequence number block forever.

Two failure scenarios:
1. **Sequencing succeeded but commit failed**: Transaction got sequence number, but database commit failed
2. **Lock acquisition failed**: Couldn't get sequence lock, and commit also failed

**Pattern**: Missing notification on error path / Lost wakeup

## Bug Pattern

```
NORMAL FLOW
-----------
1. Assign sequence number (e.g., seq=2)
2. Commit to database
3. Notify batch notifier: "seq 2 is done"
4. Waiters for seq 2+ can proceed

BUGGY FLOW (commit fails)
-------------------------
1. Assign sequence number (seq=2)
2. Commit to database -> FAILS!
3. Return error (NO notification!)
4. Waiters for seq 2+ BLOCK FOREVER!

Gap in sequence chain breaks all downstream waiters.
```

## Reproduction

### Buggy Version
```bash
cargo run
```

**Expected Output**:
```
=== Sui PR #5868: Batch Notifier Missing Notification ===

Running BUGGY version (missing notification on failure)...

Scenario: Commit tx1 (success), tx2 (fail), tx3 (success)
Problem: Waiter for seq 2 blocks forever

[BUGGY] Assigned sequence 1 to tx1
[NOTIFIER] Notified sequence 1
[BUGGY] Assigned sequence 2 to tx2
[BUGGY] Commit failed for tx2 (seq 2), NOT notifying!
[BUGGY] Assigned sequence 3 to tx3
[NOTIFIER] Notified sequence 3

Highest notified sequence: 3
(Should be 3, but tx2's failure broke the chain)

Waiting for sequence 2 (2 second timeout)...

=== Results ===
[BUG DEMONSTRATED]
Wait for sequence 2 timed out!

Problem:
  - tx2 was assigned sequence 2
  - tx2's commit failed
  - Notifier never told about sequence 2
  - Waiters for seq 2+ block forever!
```

### Fixed Version
```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui PR #5868: Batch Notifier Missing Notification ===

Running FIXED version (always notify)...

Scenario: Commit tx1 (success), tx2 (fail), tx3 (success)
Fix: Always notify sequence, even on failure

[FIXED] Assigned sequence 1 to tx1
[NOTIFIER] Notified sequence 1
[FIXED] Commit succeeded for tx1 (seq 1)
[FIXED] Assigned sequence 2 to tx2
[NOTIFIER] Notified sequence 2
[FIXED] Commit failed for tx2 (seq 2), but notified anyway
[FIXED] Assigned sequence 3 to tx3
[NOTIFIER] Notified sequence 3
[FIXED] Commit succeeded for tx3 (seq 3)

Highest notified sequence: 3

Waiting for sequence 2...

=== Results ===
[FIXED]
Wait for sequence 2 succeeded!

Fix: Notify batch notifier even on commit failure
```

## Fix Strategy

### BUGGY: Only Notify on Success
```rust
fn commit_certificate(&self, digest: &str) -> Result<SequenceNumber, Error> {
    let seq = self.assign_sequence();

    match self.database.commit(digest, seq) {
        Ok(()) => {
            self.notifier.notify_sequence(seq);  // Only here!
            Ok(seq)
        }
        Err(e) => {
            // BUG: No notification! Sequence gap!
            Err(e)
        }
    }
}
```

### FIXED: Always Notify
```rust
fn commit_certificate(&self, digest: &str) -> Result<SequenceNumber, Error> {
    let seq = self.assign_sequence();

    let result = self.database.commit(digest, seq);

    // FIX: Always notify, regardless of commit result
    self.notifier.notify_sequence(seq);

    result.map(|_| seq)
}
```

## Distributed System Relevance

This bug is critical for:
- **Consensus systems**: Sequence number tracking in Raft/Paxos
- **Message queues**: Offset tracking in Kafka-style systems
- **Databases**: LSN (Log Sequence Number) tracking
- **Event sourcing**: Event sequence tracking

**Real-world impact in Sui**:
- Authority could get stuck
- Dependent operations blocked indefinitely
- Required careful placement of notification calls

## Tool Detection

- **Testing**: Would require failure injection to trigger
- **Model checking**: Could detect missing notifications
- **Code review**: Pattern of notify-only-on-success is suspicious
- **Fault injection**: Would expose the blocking behavior

## Notes

- This is a classic **lost wakeup** bug pattern
- The fix ensures notification happens on ALL paths
- Similar to "always release lock in finally block" pattern
- Even failed operations must update shared state trackers
- Sequence gaps are toxic to monotonic progress tracking
