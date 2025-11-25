# GreptimeDB PR #3771: Region Guard Not Released on Procedure Completion

## Bug Information
- **Source**: GreptimeDB (Time Series Database)
- **PR**: https://github.com/GreptimeTeam/greptimedb/pull/3771
- **Type**: Non-blocking bug (Data Race / Resource Leak)
- **Category**: Guard lifecycle / Stale state

## Root Cause

When a table drop procedure completes, the guard marking the region as "dropping" was not released. This causes:

1. The region remains marked as "dropping" even though it's already dropped
2. Subsequent operations see inconsistent state
3. New table creation with the same region ID is blocked forever

Quote from PR: *"the procedure returned does not mean it's dropped"* - The implementation incorrectly assumed procedure completion meant cleanup was done.

**Pattern**: Resource guard not released / Stale state leak

## Bug Pattern

```
DROP TABLE PROCEDURE
--------------------
1. Create dropping_region guard (marks region as "dropping")
2. Execute drop logic
3. Procedure returns SUCCESS
4. Guard NOT released! (still shows "dropping")

SUBSEQUENT OPERATION
--------------------
5. Check if region is dropping -> YES (stale!)
6. Block/fail operation
7. But region was already dropped!

INCONSISTENT STATE!
```

## Reproduction

### Buggy Version
```bash
cargo run
```

**Expected Output**:
```
=== GreptimeDB PR #3771: Region Guard Not Released ===

Running BUGGY version (guard leaked)...

Scenario: Drop table procedure followed by region check

[BUGGY] Starting drop procedure for region 1
[BUGGY] Region 1 marked as dropping
[BUGGY] Region 1 data dropped
[BUGGY] Procedure returned (guard NOT released!)

Checking region state after procedure completed...

=== Results ===
Region 1 still marked as dropping: true
Region 1 actually valid: false

[BUG DEMONSTRATED]
Inconsistent state!
  - Guard says region is being dropped (still held)
  - But region was already dropped!
  - New operations will be blocked forever

This causes data races when:
  1. New table creation with same region ID blocked
  2. Queries see 'dropping' state indefinitely
```

### Fixed Version
```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== GreptimeDB PR #3771: Region Guard Not Released ===

Running FIXED version (guard released on completion)...

Scenario: Drop table procedure followed by region check

[FIXED] Starting drop procedure for region 1
[FIXED] Region 1 marked as dropping
[FIXED] Region 1 data dropped
[FIXED] Guard released, region no longer marked as dropping

Checking region state after procedure completed...

=== Results ===
Region 1 still marked as dropping: false
Region 1 actually valid: false

[FIXED]
Consistent state!
  - Guard properly released (not dropping)
  - Region correctly marked as dropped
  - New operations can proceed
```

## Fix Strategy

### BUGGY: Guard Not Released
```rust
fn execute(&self, region_id: RegionId) -> bool {
    let guard = DroppingRegionGuard::new(region_id);

    // Do work...
    self.store.drop_region(region_id);

    // BUG: Guard held elsewhere, not dropped!
    std::mem::forget(guard);  // Or stored in long-lived structure

    true  // Procedure "completed" but guard leaked
}
```

### FIXED: Guard Explicitly Released
```rust
fn execute(&self, region_id: RegionId) -> bool {
    let mut guard = DroppingRegionGuard::new(region_id);

    // Do work...
    self.store.drop_region(region_id);

    // FIX: Explicitly release before returning
    guard.release();

    true
}
```

## Distributed System Relevance

This bug is critical for:
- **Databases**: Table/partition lifecycle management
- **Distributed storage**: Region migration, splitting, merging
- **Cloud services**: Resource cleanup on procedure completion
- **State machines**: Guard/lock lifecycle in distributed procedures

**Real-world impact in GreptimeDB**:
- Tables stuck in "dropping" state indefinitely
- New table creation blocked for same region IDs
- Detected via sqlness test failures

## Tool Detection

- **Clippy**: May detect potential resource leaks
- **lockbud**: Could detect guard not released patterns
- **Testing**: Integration tests can catch state inconsistencies
- **Tracing**: Would show guards not being dropped

## Notes

- This is a **resource lifecycle** bug
- Guards must be released when the operation they protect completes
- Similar to "missing unlock" but for custom guards
- The fix ensures guard release is tied to procedure completion
- Same pattern affected multiple procedures (PR #3775)
