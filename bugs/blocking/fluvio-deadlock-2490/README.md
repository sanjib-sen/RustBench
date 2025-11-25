# Fluvio PR #2490: Write Lock Held Across Async IO

## Bug Information
- **Source**: Fluvio (Distributed Streaming Platform)
- **PR**: https://github.com/infinyon/fluvio/pull/2490
- **Type**: Blocking bug (Lock Starvation)
- **Category**: Lock held across await / IO blocking

## Root Cause

Write lock guard was not dropped before async IO operations. Under high load, the await time for IO could be significant, and during all that time the lock guard blocks all other readers and writers from making progress.

Quote from PR: *"Write lock guard shouldn't outlive `await` on async IO operations because under high load awaiting time could be significant, and during all that time, the lock guard won't be dropped and will prevent the system from any progress."*

**Pattern**: Lock held across async boundary / IO starvation

## Bug Pattern

```
BUGGY: Lock held during IO
--------------------------
acquire_write_lock()
update_cache()
await write_to_disk()  // Lock still held!
release_lock()

// All readers blocked for entire IO duration!

FIXED: Lock released before IO
------------------------------
acquire_write_lock()
update_cache()
data = clone_data()
release_lock()         // Released early!
await write_to_disk()  // IO without lock

// Readers can proceed during IO!
```

## Reproduction

### Buggy Version
```bash
cargo run
```

**Expected Output**:
```
=== Fluvio PR #2490: Write Lock Across Async IO ===

Running BUGGY version (lock held during IO)...

Scenario: Writer updates cache and persists to disk
Problem: Lock held during entire IO operation blocks readers

[BUGGY] Acquiring write lock...
[BUGGY] Got write lock, updating cache...
[BUGGY] Persisting to disk (holding lock)...
    [IO] Writing 5 bytes...
[BUGGY] Reader: trying to acquire read lock...
    [IO] Write complete
[BUGGY] Done, releasing lock
[BUGGY] Reader: got lock after 450ms, version=1

=== Results ===
[BUG DEMONSTRATED]
Reader blocked for 450ms waiting for write lock!

Problem: Write lock held during 500ms IO operation
  - Writer holds lock: acquire -> update -> IO -> release
  - Reader blocked: ~500ms waiting for lock

In real async code, this causes complete starvation under load.
```

### Fixed Version
```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Fluvio PR #2490: Write Lock Across Async IO ===

Running FIXED version (lock released before IO)...

Scenario: Writer updates cache and persists to disk
Fix: Lock released before slow IO operation

[FIXED] Acquiring write lock...
[FIXED] Got write lock, updating cache...
[FIXED] Cache updated, releasing lock before IO...
[FIXED] Persisting to disk (lock released)...
    [IO] Writing 5 bytes...
[FIXED] Reader: trying to acquire read lock...
[FIXED] Reader: got lock after 0ms, version=1
    [IO] Write complete
[FIXED] Done

=== Results ===
[FIXED]
Reader blocked for only 0ms!

Fix: Lock released before IO operation
  - Writer: acquire -> update -> release -> IO
  - Reader: can acquire lock during IO phase

No starvation, concurrent access works properly.
```

## Fix Strategy

### BUGGY: Lock Held During IO
```rust
fn write_and_persist(&self, data: Vec<u8>) {
    let mut cache = self.cache.write().unwrap();
    cache.update(data);

    // BUG: Lock still held during slow IO!
    async_write_to_disk(cache.get_data()).await;

    // Lock released here - too late!
}
```

### FIXED: Lock Released Before IO
```rust
fn write_and_persist(&self, data: Vec<u8>) {
    let data_to_persist;
    {
        let mut cache = self.cache.write().unwrap();
        cache.update(data);
        data_to_persist = cache.get_data().to_vec();
        // Lock released here!
    }

    // FIX: IO happens without holding the lock
    async_write_to_disk(&data_to_persist).await;
}
```

## Distributed System Relevance

This bug is critical for:
- **Streaming systems**: Kafka, Pulsar, Fluvio (producer/consumer locks)
- **Async Rust services**: Any service using RwLock with async IO
- **Database connections**: Connection pool locks during queries
- **Cache systems**: Read-through caches with slow backends
- **Network services**: Lock contention during network calls

**Real-world impact in Fluvio**:
- System throughput degraded under high load
- Hourly tests failing due to timeout
- Readers starved while writers performed IO

## Tool Detection

- **Clippy**: Has lint for holding locks across await (async code)
- **lockbud**: May detect long lock hold patterns
- **Profiling**: Would show lock contention under load
- **Tokio Console**: Would show task blocking

## Notes

- This is a common pattern in async Rust code
- The fix is to minimize critical section duration
- Clone data if needed to release lock early
- In async code: `MutexGuard` cannot be held across `.await`
- Rust's borrow checker doesn't prevent this in sync code
- Similar to "don't do IO while holding a lock" principle
