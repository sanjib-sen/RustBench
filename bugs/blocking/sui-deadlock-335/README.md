# Sui Issue #335: Absence of Proper Locking

## Bug Information
- **Source**: Sui Blockchain (MystenLabs)
- **Issue**: https://github.com/MystenLabs/sui/issues/335
- **Type**: Blocking bug (Missing synchronization / Conflict)
- **Category**: Absence of locking mechanism

## Root Cause

Absence of proper locking, causing simultaneous conflicting orders to be submitted and processed. Multiple transactions can try to use the same owned object, leading to conflicts where both transactions may execute (non-deterministic outcome) or cause blocking.

**Pattern**: Missing lock / Concurrent access to exclusive resource

## Bug Pattern

```
BUGGY Behavior:
--------------
Order 1: Use obj_001              Order 2: Use obj_001
------------------------          ------------------------
1. Check obj_001 exists           1. Check obj_001 exists
2. Add to pending                 2. Add to pending
3. Process...                     3. Process...
4. "Acquire" obj_001              4. "Acquire" obj_001  <- CONFLICT!
5. Execute                        5. Execute <- BOTH EXECUTE!

Both orders execute on the same object!
Last writer wins (non-deterministic outcome)

FIXED Behavior:
--------------
Order 1: Use obj_001              Order 2: Use obj_001
------------------------          ------------------------
1. Acquire lock on obj_001        1. Try to acquire lock
2. Process...                     2. WAIT (obj_001 locked)
3. Execute                        3. ...waiting...
4. Release lock                   4. Acquire lock
                                  5. Process...
                                  6. Execute
                                  7. Release lock

Orders execute sequentially, no conflict.
```

## Reproduction

### Buggy Version
```bash
cargo run
```

**Expected Output**:
```
=== Sui Issue #335: Absence of Proper Locking ===

Running BUGGY version (no locking, allows conflicts)...

Scenario: Two orders for the same object submitted simultaneously

[BUGGY] Processing order order_001 for objects ["obj_001"]
[BUGGY] Processing order order_002 for objects ["obj_001"]
[BUGGY] CONFLICT! Object obj_001 has multiple orders: ["order_001", "order_002"]
[BUGGY] Order order_001 acquired object obj_001
[BUGGY] CONFLICT! Object obj_001 has multiple orders: ["order_001", "order_002"]
[BUGGY] Order order_002 acquired object obj_001

=== Results ===
Order 1 result: Success
Order 2 result: Success
Final object holder: Some("order_002")

[BUG DEMONSTRATED]
Both conflicting orders succeeded!

Problem:
  - No locking mechanism to prevent conflicts
  - Both orders executed on the same object
  - Last writer wins (non-deterministic)
```

### Fixed Version
```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui Issue #335: Absence of Proper Locking ===

Running FIXED version (proper locking)...

Scenario: Two orders for the same object submitted simultaneously

[FIXED] Processing order order_001 for objects ["obj_001"]
[FIXED] Order order_001 acquired lock on obj_001
[FIXED] Processing order order_002 for objects ["obj_001"]
[FIXED] Order order_002 waiting for obj_001 (locked by Some("order_001"))
[FIXED] Order order_001 executed on obj_001
[FIXED] Order order_001 released lock on obj_001
[FIXED] Order order_002 acquired lock on obj_001
[FIXED] Order order_002 executed on obj_001
[FIXED] Order order_002 released lock on obj_001

=== Results ===
Order 1 result: Success
Order 2 result: Success
Final object holder: Some("order_002")

[FIXED - Sequential Execution]
Both orders succeeded sequentially!

Fix: Proper locking ensures serial execution
  - First order acquires lock
  - Second order waits for lock
  - Orders execute one at a time
```

## Fix Strategy

### BUGGY: No Locking
```rust
fn handle_order(&self, order: &Order) -> OrderResult {
    // Check if objects exist
    // ... no lock!

    // Add to pending (not a real lock)
    pending.entry(obj_id).or_insert(vec![]).push(order.digest);

    // Process (opens race window)
    thread::sleep(processing_time);

    // Check for conflicts (too late!)
    if pending[obj_id].len() > 1 {
        // Already processing, conflict detected after the fact
    }

    // Execute (both conflicting orders may reach here!)
    obj.locked_by = Some(order.digest);
}
```

### FIXED: Proper Locking
```rust
fn handle_order(&self, order: &Order) -> OrderResult {
    // Try to acquire locks on all input objects
    for obj_id in &order.input_objects {
        loop {
            let mut locks = self.object_locks.lock().unwrap();
            if lock_entry.locked_by.is_none() {
                // FIX: Acquire exclusive lock before processing
                lock_entry.locked_by = Some(order.digest);
                break;
            } else {
                // FIX: Wait for lock to be released
                cvar.wait(...);
            }
        }
    }

    // Process (now safe - we hold the lock)
    thread::sleep(processing_time);

    // Execute
    obj.locked_by = Some(order.digest);

    // FIX: Release locks and wake waiters
    self.release_locks(&order.digest, &acquired_locks);
}
```

## Distributed System Relevance

This bug is critical for:
- **Blockchain systems**: Transaction ordering and object ownership
- **Database systems**: Row-level locking, optimistic vs pessimistic locking
- **Distributed locks**: Resource contention in distributed systems
- **Concurrent data structures**: Exclusive access to shared resources

**Real-world impact in Sui**:
- Conflicting transactions could both execute
- Object state becomes non-deterministic
- Potential for double-spending or inconsistent state
- Required adding locking access to the table

## Tool Detection

- **Testing**: Concurrent tests could detect conflicts
- **Model checking**: Would detect missing synchronization
- **Static analysis**: Could identify unprotected shared state access
- **Fuzzing**: Random ordering could expose conflicts

## Notes

- This is a **fundamental synchronization bug**
- The fix adds proper locking with wait/notify mechanism
- Similar to database pessimistic locking
- Critical for maintaining ACID properties in transactions
- "Absence of locking" is a common root cause of concurrency bugs
