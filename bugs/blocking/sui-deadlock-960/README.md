# Sui Deadlock #960: Object Lock Deadlock

## Bug Information
- **Source**: Sui Blockchain (REST Server / Gateway)
- **Issue**: https://github.com/MystenLabs/sui/issues/960
- **Type**: Blocking bug (Deadlock)
- **Category**: Missing unlock on error path / Resource leak deadlock

## Root Cause

The Sui REST server's gateway state implementation locked transaction objects but failed to unlock them when transaction execution encountered errors (network failures, broken pipes, etc.). This left objects permanently locked, preventing subsequent transactions from acquiring locks on the same objects.

**Manifestation during GenITeam Monstars game demo:**
- Clients received HTTP 424 errors: *"Client state has a different pending transaction"*
- Service became unavailable as objects remained locked
- New transactions deadlocked trying to acquire already-locked objects

**Pattern**: Missing cleanup on error path causing resource deadlock

## Bug Pattern (Abstracted)

```
Transaction 1                          Transaction 2
-------------                          -------------
lock_objects(A, B)
execute_transaction()
  -> Network Error!
  -> return error
  -> BUG: forget to unlock!
(objects A, B remain locked)
                                      lock_objects(A, B)
                                        -> DEADLOCK!
                                        -> A, B still locked by TX1
                                        -> Return 424 error
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== Sui Issue #960: Object Lock Deadlock ===

Running BUGGY version (missing unlock on error)...

--- Transaction 1 (will fail) ---
[BUGGY] Executing transaction "tx_1_fail"
  [LOCK] Object "object_A" locked by transaction "tx_1_fail"
[BUGGY] Transaction "tx_1_fail" failed: NetworkError("Broken pipe")
[BUGGY] WARNING: Objects remain locked! (unlock not called)

[BUG DETECTED] Object "object_A" is still locked!

--- Transaction 2 (will deadlock) ---
[BUGGY] Attempting transaction "tx_2"
[BUGGY] DEADLOCK! Transaction blocked - object "object_A" still locked from previous failed transaction

=== Results ===
[BUG DEMONSTRATED]
First transaction failed and left object locked.
Second transaction deadlocked trying to acquire the same lock.
In production, this causes HTTP 424 errors: 'Client state has a different pending transaction'

Run with --fixed to see proper unlock handling.
```

**What Happens**:
1. Transaction 1 locks object A
2. Transaction 1 fails with network error
3. **BUG**: Unlock is not called on error path
4. Object A remains locked forever
5. Transaction 2 tries to lock object A
6. Transaction 2 deadlocks because A is still locked
7. Clients get HTTP 424 error

### Running the Fixed Version

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui Issue #960: Object Lock Deadlock ===

Running FIXED version (unlock on all paths)...

--- Transaction 1 (will fail) ---
[FIXED] Executing transaction "tx_1_fail"
  [LOCK] Object "object_A" locked by transaction "tx_1_fail"
  [UNLOCK] Object "object_A" unlocked
[FIXED] Transaction "tx_1_fail" failed: NetworkError("Broken pipe")
[FIXED] Objects properly unlocked despite error

[FIXED] Object "object_A" properly unlocked

--- Transaction 2 (should succeed) ---
[FIXED] Executing transaction "tx_2"
  [LOCK] Object "object_A" locked by transaction "tx_2"
  [UNLOCK] Object "object_A" unlocked
[FIXED] Transaction "tx_2" succeeded
[FIXED] Transaction completed successfully!
[FIXED] No deadlock - object was properly released

=== Results ===
[FIXED]
First transaction failed but properly unlocked objects.
Second transaction succeeded - no deadlock.
Unlock is guaranteed on all code paths (success and error).
```

**What Happens**:
1. Transaction 1 locks object A
2. Transaction 1 fails with network error
3. **FIX**: Unlock is called despite error
4. Object A is released
5. Transaction 2 successfully locks object A
6. Transaction 2 completes successfully
7. No deadlock!

## Fix Strategy

### Approach 1: Explicit unlock on all paths
```rust
// BUGGY: Unlock only on success
pub fn execute_transaction(&self, objects: Vec<ObjectId>) -> Result<(), Error> {
    self.lock_objects(&objects)?;
    let result = execute()?;
    if result.is_ok() {
        self.unlock_objects(&objects); // BUG: Missing on error path!
    }
    result
}

// FIXED: Always unlock
pub fn execute_transaction(&self, objects: Vec<ObjectId>) -> Result<(), Error> {
    self.lock_objects(&objects)?;
    let result = execute();
    self.unlock_objects(&objects); // FIX: Always called
    result
}
```

### Approach 2: RAII Guard Pattern (Rust idiomatic)
```rust
// Best practice: Use guard that auto-unlocks on drop
struct ObjectLockGuard<'a> {
    manager: &'a LockManager,
    objects: Vec<ObjectId>,
}

impl Drop for ObjectLockGuard<'_> {
    fn drop(&mut self) {
        self.manager.unlock_objects(&self.objects);
    }
}

pub fn execute_transaction(&self, objects: Vec<ObjectId>) -> Result<(), Error> {
    let _guard = self.lock_objects(&objects)?; // Auto-unlocks on drop
    execute() // Unlock happens automatically, even on error/panic
}
```

### Approach 3: Remove lock/unlock (Sui's temporary fix)
The Sui team temporarily removed the lock/unlock mechanism entirely for their demo, indicating the fundamental issue with manual resource management.

## Distributed System Relevance

This bug pattern is critical for:
- **Blockchain transaction pools**: Ethereum, Bitcoin, Solana all manage transaction locks
- **Database transaction managers**: PostgreSQL, MySQL lock row resources
- **Distributed lock services**: Zookeeper, etcd lease management
- **Workflow orchestration**: Temporal, Airflow resource locking
- **Multi-threaded servers**: Connection pools, request handlers
- **File systems**: Advisory file locking

**Real-world consequences**:
- Service unavailability (HTTP 424 errors)
- Cascading failures across distributed systems
- Requires service restart to clear stuck locks
- Can affect hundreds of users simultaneously

## Tool Detection

- **lockbud**: May detect missing unlock patterns
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (locks are correctly used, just not released)
- **Clippy**: May warn about unused Result in error cases
- **Static analysis**: Could detect unbalanced lock/unlock calls
- **Runtime deadlock detector**: Would show lock never released

## Notes

- This is a **classic deadlock** from missing cleanup
- Rust's ownership system doesn't prevent this (locks are runtime constructs)
- **Best practice**: Always use RAII (guards) for resource management
- The bug was visible during a live demo, causing immediate user impact
- HTTP 424 "Failed Dependency" is appropriate error code for this scenario
- Related to issues #335 and #346 in Sui repository
- The temporary fix was to remove locking entirely, showing architectural issues
