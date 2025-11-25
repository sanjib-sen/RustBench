# Ballista Deadlock #132: Executor Task Slot Deadlock

## Bug Information
- **Source**: Apache Ballista (Distributed Query Engine)
- **Issue**: https://github.com/apache/datafusion-ballista/issues/132
- **Type**: Blocking bug (Deadlock)
- **Category**: Resource starvation / Dependency inversion deadlock

## Root Cause

When tasks from stage 2 are scheduled via round-robin distribution, they occupy executor slots while waiting for their input partitions from stage 1. If all executor slots are filled with stage 2 tasks, stage 1 tasks cannot execute, causing system deadlock.

Quote from issue: *"if a task from stage 2 is scheduled on one executor it would block its executor slot until its input partitions start executing on their executors. Under certain conditions this could deadlock the system."*

**Pattern**: Dependency inversion deadlock / Resource starvation

## Bug Pattern

```
Executor (2 slots)
------------------
Slot 1: [Stage 2 Task A] waiting for Stage 1...
Slot 2: [Stage 2 Task B] waiting for Stage 1...

Stage 1 Task Queue: [Task C, Task D] - NO SLOTS AVAILABLE!

DEADLOCK: Stage 2 waits for Stage 1, but Stage 1 can't run!
```

## Reproduction

### Buggy Version
```bash
cargo run
```

### Fixed Version
```bash
cargo run -- --fixed
```

## Fix Strategy

Don't schedule tasks until their dependencies are complete. The fix involves:
1. Queue tasks whose dependencies aren't met
2. Only take executor slots for ready-to-run tasks
3. Process pending queue when stages complete

## Tool Detection

- **lockbud**: May detect circular wait patterns
- **Static analysis**: Could detect dependency ordering issues
