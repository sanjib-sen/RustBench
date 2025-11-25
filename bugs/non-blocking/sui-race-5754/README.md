# Sui Race #5754: Object Version Race

## Bug Information
- **Source**: Sui Blockchain
- **Issue**: https://github.com/MystenLabs/sui/issues/5754
- **Fix PR**: https://github.com/MystenLabs/sui/pull/7044
- **Type**: Non-blocking bug (Race Condition)
- **Category**: Version tracking race / Stale state

## Root Cause

During epoch initialization, the code reads object versions from `parent_sync` table. However, checkpoint execution can update this table concurrently, causing two problems:

1. **New Validator Join**: When a validator joins mid-epoch with empty sequence tables, it must determine the next version. The `get_latest_parent_entry()` value might be stale if checkpoint sync executed transactions ahead.

2. **Owned-to-Shared Upgrades**: When an owned object transitions to shared, the system should use `initial_shared_version`. But the buggy code always prioritizes parent_sync entry, ignoring the initial version for upgraded objects.

**Pattern**: TOCTOU race on version state + ignoring fallback values

## Bug Pattern (Abstracted)

```
Thread 1 (Epoch Init)              Thread 2 (Checkpoint Sync)
---------------------              ------------------------
read parent_sync(obj)
  -> version = 0 (empty)
  -> use version 1
                                   execute checkpoint
                                   update parent_sync(obj) = 150
process with version 1
  -> WRONG! Should be 101+

Meanwhile:
- initial_shared_version(obj) = 100
- But this was never consulted!
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== Sui Issue #5754: Object Version Race ===

Running BUGGY version (stale parent_sync)...

Object 'obj_upgrade' became shared at version 100

[Thread 1] Epoch initialization starting...
[BUGGY] No parent_sync entry for 'obj_upgrade', using version 1

[Thread 2] Checkpoint sync executing...
[Thread 2] Updated parent_sync to version 150
[Thread 1] Will use version 1 for next operation

=== Results ===
Initial shared version: 100
Final parent_sync version: 150
Epoch chose version: 1

[BUG DEMONSTRATED]
Epoch initialization used stale version 1!
Should have used at least 101 (initial_shared_version)
This can cause version conflicts in transaction processing.
```

**What Happens**:
- Object became shared at version 100
- Epoch init reads parent_sync → empty (returns version 1)
- Checkpoint updates parent_sync to 150
- Epoch uses version 1, ignoring both:
  - The initial_shared_version (100)
  - The checkpoint update (150)
- **Result**: Version conflict, wrong sequencing!

### Running the Fixed Version

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui Issue #5754: Object Version Race ===

Running FIXED version (max of versions)...

Object 'obj_upgrade' became shared at version 100

[Thread 1] Epoch initialization starting...
[FIXED] Object 'obj_upgrade': parent_sync=0, initial_shared=100, using max=100

[Thread 2] Checkpoint sync executing...
[Thread 2] Updated parent_sync to version 150
[Thread 1] Will use version 101 for next operation

=== Results ===
Initial shared version: 100
Final parent_sync version: 150
Epoch chose version: 101

[FIXED]
Epoch initialization correctly used version 101!
Used max(parent_sync, initial_shared) to ensure consistency.
No version conflicts possible.
```

**What Happens**:
- Uses `max(parent_sync_version, initial_shared_version)`
- Even with empty parent_sync, uses initial_shared_version (100)
- Returns version 101
- **Result**: Correct sequencing, no conflicts!

## Fix Strategy

### BUGGY: Only Check parent_sync
```rust
pub fn get_next_version(&self, object_id: &str) -> Version {
    // BUG: Ignores initial_shared_version!
    if let Some(obj_ref) = self.parent_sync.get_latest_parent_entry(object_id) {
        obj_ref.version + 1
    } else {
        1 // Wrong for upgraded objects!
    }
}
```

### FIXED: Use max() of Both Sources
```rust
pub fn get_next_version(&self, object_id: &str) -> Version {
    let initial_version = self.shared_objects
        .get_initial_shared_version(object_id)
        .unwrap_or(0);

    let parent_version = self.parent_sync
        .get_latest_parent_entry(object_id)
        .map(|r| r.version)
        .unwrap_or(0);

    // FIX: max() handles all cases:
    // - If already shared: parent_sync >= initial_shared
    // - If absent: use initial_shared
    max(parent_version, initial_version) + 1
}
```

**Why max() Works**:
- If object is already shared in parent_sync: `parent_version >= initial_version`
- If object just upgraded (not in parent_sync): use `initial_version`
- No need to read object table or track complex state

## Distributed System Relevance

This bug is critical for:
- **Blockchain state machines**: Ethereum, Solana (object/account versioning)
- **Distributed databases**: CockroachDB, TiDB (MVCC versions)
- **Event sourcing systems**: Event sequence numbers
- **Distributed caches**: Cache invalidation versions
- **Consensus protocols**: Log sequence numbers
- **Replicated state machines**: Operation ordering

**Real-world impact in Sui**:
- Version conflicts during validator joins
- Incorrect sequencing for upgraded objects
- Potential transaction failures or reordering

## Tool Detection

- **lockbud**: Unlikely to detect (no explicit lock bug)
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (correct Rust semantics)
- **Static analysis**: Could detect missing fallback check
- **Testing**: Would require specific race timing to trigger

## Notes

- This is a **state machine race** bug
- The fix is elegant: `max()` makes the operation **monotonic**
- Monotonic operations are naturally race-safe
- Similar to "last-write-wins" conflict resolution in CRDTs
- The bug demonstrates importance of **idempotent version selection**
