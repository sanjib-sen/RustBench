# Sui Race #4990: Parallel Certificate Execution Race

## Bug Information
- **Source**: Sui Blockchain
- **Issue**: https://github.com/MystenLabs/sui/issues/4990
- **Fix PR**: https://github.com/MystenLabs/sui/pull/5778
- **Type**: Non-blocking bug (Race Condition)
- **Category**: Dependency ordering race / Parallel execution without coordination

## Root Cause

In Sui's execution driver, certified transactions (certificates) are sent to `NodeSyncState` for parallel execution without considering dependencies between them. While certificates arrive in order from consensus, the parallel execution layer is "unaware of dependencies among its inputs."

This causes a race condition where:
1. Certificates A and B arrive in sequence (A before B)
2. Both are dispatched for parallel execution
3. Certificate B may execute before A completes
4. B fails because it needs A's output objects, which don't exist yet
5. System incorrectly determines parent certificates are missing
6. Validators repeatedly request certificate effects from peers, cascading across the network

**Pattern**: Dependency ordering violation in parallel task execution

## Bug Pattern (Abstracted)

```
Thread 1 (Task A)              Thread 2 (Task B)
-----------------              -----------------
execute(A)
  needs obj_0 ✓
  computing...                 execute(B)
                                 needs obj_1 (produced by A)
                                 obj_1 not ready!
                                 FAIL - missing parent!
  produces obj_1
  complete
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== Sui Issue #4990: Parallel Certificate Execution Race ===

Running BUGGY version (parallel execution without dependency tracking)...

[BUGGY] Executing task B
[BUGGY] Executing task A
[BUGGY] Executing task C
[BUGGY] Task B FAILED! Missing input "obj_1"
[BUGGY] Task C FAILED! Missing input "obj_2"
[BUGGY] Task A completed successfully

=== Results ===
Task A: SUCCESS
Task B: FAILED ([BUGGY] Task B FAILED! Missing input "obj_1")
Task C: FAILED ([BUGGY] Task C FAILED! Missing input "obj_2")

[BUG DEMONSTRATED]
2 task(s) failed due to parallel execution without dependency tracking.
Tasks raced ahead before their dependencies completed.
```

**What Happens**:
- Tasks A, B, and C are submitted to parallel executor
- Tasks B and C race ahead before A completes
- They fail because their input objects (produced by earlier tasks) don't exist yet
- This mirrors the real bug where certificates fail unnecessarily

### Running the Fixed Version

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui Issue #4990: Parallel Certificate Execution Race ===

Running FIXED version (dependency-aware execution)...

[FIXED] Task B waiting for dependencies, queuing...
[FIXED] Task C waiting for dependencies, queuing...
[FIXED] Executing task A
[FIXED] Task A completed successfully
[FIXED] Executing previously pending task B
[FIXED] Task B completed successfully
[FIXED] Executing previously pending task C
[FIXED] Task C completed successfully

=== Results ===
Task A: SUCCESS
Task B: SUCCESS
Task C: SUCCESS

[FIXED]
All tasks completed successfully with dependency tracking.
Tasks waited for their dependencies before executing.
```

**What Happens**:
- Tasks check if dependencies are ready before executing
- Tasks B and C queue themselves when dependencies aren't ready
- After A completes, B executes
- After B completes, C executes
- All tasks succeed in the correct order

## Fix Strategy

The fix implements **partial ordering** of certificates before execution:

1. **Track dependencies**: Maintain mappings between pending certificates and their input object requirements
2. **Execution driver**: Process certificates only when all inputs are locally available
3. **Atomic batch writes**: In consensus handling, take shared locks and add certificates to pending table in one atomic batch
4. **Crash recovery**: Use `pending_certificates` table as durable replay log

### Key Code Pattern

```rust
// BUGGY: Execute immediately without checking dependencies
pub fn execute_task(&self, task: Task) {
    // No dependency checking!
    if !self.state.has_object(input) {
        return TaskResult::Failed("Missing input"); // Race!
    }
}

// FIXED: Check dependencies and queue if not ready
pub fn execute_task(&self, task: Task) {
    let can_execute = task.inputs.iter()
        .all(|input| self.state.has_object(input));

    if !can_execute {
        self.pending.lock().unwrap().push(task);
        return TaskResult::Failed("Pending");
    }
    // Execute and retry pending tasks
    self.try_execute_pending();
}
```

## Distributed System Relevance

This bug is critical for:
- **Blockchain transaction execution**: Ethereum, Solana, Sui all face parallel execution challenges
- **Distributed query processing**: Ballista, Spark must handle task dependencies
- **Workflow orchestration**: Airflow, Temporal need dependency resolution
- **Build systems**: Bazel, Buck handle parallel compilation with dependencies
- **Message queue processing**: Kafka, RabbitMQ with ordered message dependencies

## Tool Detection

- **lockbud**: Unlikely to detect (no explicit lock bug)
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (logic error, not undefined behavior)
- **loom**: Could potentially detect with proper scheduling exploration
- **Static analysis**: Could detect missing dependency checks with dataflow analysis

## Notes

- This is a **timing-dependent** race condition
- Some runs may not show failures if scheduling happens to be correct
- Real-world impact: Caused cascading failures across Sui validator network
- The bug demonstrates that "consensus ordering ≠ execution ordering" in distributed systems
