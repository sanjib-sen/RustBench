# Arroyo Issue #712: Task Startup Race Condition

## Bug Information
- **Source**: Arroyo (Distributed Stream Processing)
- **PR**: https://github.com/ArroyoSystems/arroyo/pull/712
- **Type**: Non-blocking bug (Race condition / State inconsistency)
- **Category**: Premature state transition

## Root Cause

A race condition in pipeline scheduling where:
1. Controller marks pipeline as "running" upon receiving TaskStarted notifications
2. TaskStarted was sent BEFORE operators completed their `on_start` phase
3. If an operator panics during startup, the TaskFailed notification is ignored
4. Pipeline appears "healthy running" but is actually non-functional

**Pattern**: Premature state transition / Ignored failure notifications

## Bug Pattern

```
BUGGY Timeline:
--------------
Task 1                      Task 2 (will panic)         Controller
------                      -------------------         ----------
send TaskStarted            send TaskStarted
                                                        recv TaskStarted(1)
                                                        recv TaskStarted(2)
                                                        state = Running
on_start()...               on_start()...
                            PANIC!
                            send TaskFailed
                                                        recv TaskFailed(2)
                                                        IGNORE! (state is Running)
on_start() done

Result: Pipeline shows "Running" but Task 2 failed!

FIXED Timeline:
--------------
Task 1                      Task 2 (will panic)         Controller
------                      -------------------         ----------
on_start()...               on_start()...
                            PANIC!
                            send TaskFailed
                                                        recv TaskFailed(2)
                                                        state = Failed
on_start() done
send TaskStarted
                                                        recv TaskStarted(1)
                                                        (state already Failed)

Result: Pipeline correctly shows "Failed"
```

## Reproduction

### Buggy Version
```bash
cargo run
```

**Expected Output**:
```
=== Arroyo Issue #712: Task Startup Race Condition ===

Running BUGGY version (TaskStarted before on_start)...

Scenario: Start 3 tasks, task 2 (transform) will panic during on_start

[BUGGY] Task 1 sending TaskStarted (before on_start)
[BUGGY] Task 2 sending TaskStarted (before on_start)
[BUGGY] Task 3 sending TaskStarted (before on_start)
[BUGGY] Received TaskStarted for task 1
[BUGGY] Received TaskStarted for task 2
[BUGGY] Received TaskStarted for task 3
[BUGGY] All tasks started, transitioning to Running
[BUGGY] Task 1 executing on_start...
[BUGGY] Task 2 executing on_start...
[BUGGY] Task 2 (transform) PANICKED during on_start!
[BUGGY] Received TaskFailed for task 2: transform panicked
[BUGGY] Ignoring failure during scheduling phase!

=== Results ===
Final pipeline state: Running

[BUG DEMONSTRATED]
Pipeline shows as 'Running' but task 2 failed!

Problem:
  - TaskStarted sent BEFORE on_start completes
  - All TaskStarted received -> pipeline marked Running
  - TaskFailed during scheduling was IGNORED
  - Pipeline appears healthy but is broken
```

### Fixed Version
```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Arroyo Issue #712: Task Startup Race Condition ===

Running FIXED version (TaskStarted after on_start)...

Scenario: Start 3 tasks, task 2 (transform) will panic during on_start

[FIXED] Task 1 executing on_start...
[FIXED] Task 2 executing on_start...
[FIXED] Task 2 (transform) PANICKED during on_start!
[FIXED] Received TaskFailed for task 2: transform panicked
[FIXED] Failure during scheduling - triggering reschedule

=== Results ===
Final pipeline state: Failed("transform panicked")

[FIXED]
Pipeline correctly shows as Failed: transform panicked

Fix:
  - TaskStarted sent AFTER on_start completes
  - TaskFailed during scheduling triggers reschedule
  - Failed tasks are properly detected
```

## Fix Strategy

### BUGGY: TaskStarted before on_start
```rust
fn start_task(&self, task: Task) {
    // BUG: Send TaskStarted BEFORE on_start completes!
    controller.notify(TaskStarted(task.id));

    // Now run on_start (might panic!)
    task.on_start();

    if panicked {
        controller.notify(TaskFailed(task.id));
        // But controller may ignore this if already "Running"!
    }
}
```

### FIXED: TaskStarted after on_start
```rust
fn start_task(&self, task: Task) {
    // FIX: Run on_start FIRST
    task.on_start();

    if panicked {
        controller.notify(TaskFailed(task.id));
        return; // Don't send TaskStarted!
    }

    // FIX: Only send TaskStarted after successful startup
    controller.notify(TaskStarted(task.id));
}
```

## Distributed System Relevance

This bug is critical for:
- **Stream processing**: Arroyo, Flink, Spark Streaming
- **Distributed pipelines**: Data transformation workflows
- **Microservices**: Service health reporting
- **Container orchestration**: Pod readiness checks

**Real-world impact in Arroyo**:
- Pipelines appear healthy but don't process data
- Monitoring shows "running" status
- Users don't realize their pipeline is broken
- Requires manual investigation to discover failure

## Tool Detection

- **Testing**: Chaos engineering with startup failures
- **Monitoring**: Health checks that verify actual operation
- **Fuzzing**: Random task failures during startup
- **Code review**: Pattern of "notify before complete" is suspicious

## Notes

- This is a classic **state machine ordering** bug
- The fix ensures notifications reflect actual state
- Similar to "readiness probe" patterns in Kubernetes
- Don't report "ready" until actually ready
- Also related: need to handle failures during all phases
