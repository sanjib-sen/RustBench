# Sui PR #3858: False Contention in Mutex Table

## Bug Information
- **Source**: Sui Blockchain (MystenLabs)
- **PR**: https://github.com/MystenLabs/sui/pull/3858
- **Type**: Blocking bug (Performance / False Contention)
- **Category**: Lock table collision / Unnecessary blocking

## Root Cause

The lock table used a fixed-size hash table where objects were indexed by their hash modulo table size. This caused **false contention** where unrelated transactions accessing different objects would block each other because they hashed to the same table slot.

Quote from PR: *"The current lock table is indexed by object/tx hash which introduces false contention since the size of this table is fixed. This false contention is quite noticeable in benchmarks and hence warranted a proper fix."*

**Pattern**: Hash table collision causing lock contention

## Bug Pattern

```
Lock Table (4 slots)
--------------------
Slot 0: [Lock] <- Objects 2, 6 compete here!
Slot 1: [Lock] <- Objects 1, 5 compete here!
Slot 2: [Lock] <- Objects 4, 8 compete here!
Slot 3: [Lock] <- Objects 3, 7 compete here!

Thread 1: Process object 1 (slot 1)
Thread 2: Process object 5 (slot 1) - BLOCKED!

Object 1 and 5 are UNRELATED but share a lock!
This is FALSE CONTENTION - unnecessary blocking.
```

## Reproduction

### Buggy Version
```bash
cargo run
```

**Expected Output**:
```
=== Sui PR #3858: False Contention in Mutex Table ===

Running BUGGY version (fixed-size table with collisions)...

Lock table size: 4 slots
Processing 8 objects across 2 threads

Object -> Slot mapping:
  Object 1 -> Slot X
  Object 2 -> Slot Y
  ...

[BUGGY] Object 1 -> slot X (hash collision possible!)
[BUGGY] Object 2 -> slot Y (hash collision possible!)
...

=== Results ===
[BUG DEMONSTRATED]
Thread 1 time: ~80ms
Thread 2 time: ~80ms
Total time: ~80-100ms (sequential due to collisions!)

Problem: Different objects collide on same lock slot!
  - Objects with same (hash % 4) block each other
  - False contention slows down parallel processing
```

### Fixed Version
```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui PR #3858: False Contention in Mutex Table ===

Running FIXED version (sharded lock table)...

Lock table: 16 shards x 16 slots = 256 possible locks
Processing 8 objects across 2 threads

[FIXED] Object 1 -> shard A, slot B (better distribution)
...

=== Results ===
[FIXED]
Thread 1 time: ~40ms
Thread 2 time: ~40ms
Total time: ~45ms (parallel - fewer collisions!)

Improvement: Sharded table reduces false contention
  - 256 possible slots vs 4 in buggy version
  - Different objects rarely collide
  - Better parallelism under high load
```

## Fix Strategy

### BUGGY: Fixed-Size Table
```rust
const TABLE_SIZE: usize = 4;

fn acquire(&self, object_id: ObjectId) {
    let slot = hash(object_id) % TABLE_SIZE;
    // Many different objects map to same slot!
    self.slots[slot].lock()
}
```

### FIXED: Sharded Table
```rust
const NUM_SHARDS: usize = 16;
const SHARD_SIZE: usize = 16;

fn acquire(&self, object_id: ObjectId) {
    let hash = hash(object_id);
    let shard = hash % NUM_SHARDS;
    let slot = (hash >> 16) % SHARD_SIZE;
    // 256 possible locks = much fewer collisions
    self.shards[shard][slot].lock()
}
```

## Distributed System Relevance

This bug is critical for:
- **Blockchain validators**: Processing concurrent transactions
- **Database systems**: Table/row-level locking
- **Concurrent data structures**: Hash-based lock striping
- **Thread pools**: Work distribution with shared resources

**Real-world impact in Sui**:
- Noticeable performance degradation in benchmarks
- Gets worse as transactions have more input objects
- Affects validator throughput under load

## Tool Detection

- **Profiling**: Would show lock contention hotspots
- **Benchmarking**: Performance degrades under concurrent load
- **Flame graphs**: Would show threads waiting on same locks
- **Lock contention analyzers**: Would identify false sharing

## Notes

- This is a **performance bug**, not a correctness bug
- The fix is a classic "lock striping" optimization
- Similar to ConcurrentHashMap's segment locking in Java
- Trade-off: more locks = more memory, less contention
- Common in high-throughput systems
