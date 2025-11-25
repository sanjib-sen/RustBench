# Sui Race #303: Non-Atomic Read-Modify-Write (Lost Update)

## Bug Information
- **Source**: Sui Blockchain (Client API)
- **Issue**: https://github.com/MystenLabs/sui/issues/303
- **Type**: Non-blocking bug (Atomicity Violation)
- **Category**: Lost update / Non-atomic read-modify-write race

## Root Cause

The Sui client API used "simple data structures" without proper synchronization for concurrent operations. Specifically, the `pending_orders` table (critical for preventing double-spending) was susceptible to race conditions during the transition to support concurrent multi-object transactions.

The bug manifests as **non-atomic read-modify-write sequences**:
1. Thread A reads current value
2. Thread B reads current value (same)
3. Thread A computes new value and writes
4. Thread B computes new value and writes (overwrites A's update!)

**Result**: Thread A's update is lost - a classic "lost update" problem.

**Pattern**: Atomicity violation causing lost updates

## Bug Pattern (Abstracted)

```
Thread 1                       Thread 2
--------                       --------
read(pending) -> 100
compute: 100 + 50 = 150        read(pending) -> 100
                               compute: 100 + 75 = 175
write(150)
                               write(175)  <-- OVERWRITES 150!

Expected: 100 + 50 + 75 = 225
Actual: 175 (lost 50!)
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== Sui Issue #303: Non-Atomic Read-Modify-Write (Lost Update) ===

Running BUGGY version (non-atomic read-modify-write)...

[BUGGY] Thread 0 adding order...
[BUGGY] Thread 1 adding order...
[BUGGY] Thread 2 adding order...
[BUGGY] Added 100 to account 'alice' (read: 0, wrote: 100)
[BUGGY] Added 100 to account 'alice' (read: 0, wrote: 100)
[BUGGY] Added 100 to account 'alice' (read: 100, wrote: 200)
[BUGGY] Thread 3 adding order...
[BUGGY] Added 100 to account 'alice' (read: 100, wrote: 200)
...

=== Results ===
Expected total: 1000
Actual total: 600
[BUGGY] LOST UPDATE! Account 'alice': expected 1000, actual 600 (lost: 400)

[BUG DEMONSTRATED]
Lost 400 units due to non-atomic read-modify-write!
This is a classic 'lost update' atomicity violation.
In Sui, this could enable double-spending attacks.
```

**What Happens**:
- 10 threads each add 100 to the same account
- Expected: 1000 total
- Multiple threads read the same old value concurrently
- They all compute based on stale data
- Later writes overwrite earlier updates
- Result: Lost updates (e.g., only 600 instead of 1000)

**Security Impact**: In a blockchain, this could allow double-spending!

### Running the Fixed Version (Mutex)

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui Issue #303: Non-Atomic Read-Modify-Write (Lost Update) ===

Running FIXED version (atomic with mutex)...

[FIXED] Thread 0 adding order...
[FIXED] Added 100 to account 'alice' (read: 0, wrote: 100)
[FIXED] Thread 1 adding order...
[FIXED] Added 100 to account 'alice' (read: 100, wrote: 200)
[FIXED] Thread 2 adding order...
[FIXED] Added 100 to account 'alice' (read: 200, wrote: 300)
...

=== Results ===
Expected total: 1000
Actual total: 1000

[FIXED]
All updates preserved! Atomic read-modify-write with Mutex.
The entire sequence is protected by a single lock.
```

**What Happens**:
- Mutex held during entire read-modify-write sequence
- No interleaving possible
- All 1000 units accounted for
- No lost updates!

### Running the Fixed Version (Atomic Operations)

```bash
cargo run -- --atomic
```

**Expected Output**:
```
=== Sui Issue #303: Non-Atomic Read-Modify-Write (Lost Update) ===

Running FIXED-ATOMIC version (atomic operations)...

[FIXED-ATOMIC] Thread 0 adding order...
[FIXED-ATOMIC] Added 100 to account 'alice' (new value: 100)
[FIXED-ATOMIC] Thread 1 adding order...
[FIXED-ATOMIC] Added 100 to account 'alice' (new value: 200)
...

=== Results ===
Expected total: 1000
Actual total: 1000

[FIXED-ATOMIC]
All updates preserved! Using AtomicU64::fetch_add.
Lock-free atomic operations ensure no updates are lost.
```

**What Happens**:
- Uses `AtomicU64::fetch_add` for lock-free increment
- Hardware-level atomic operation
- Higher performance than mutex
- All updates preserved!

## Fix Strategy

### Approach 1: Atomic Mutex Lock
```rust
// BUGGY: Release lock between read and write
let current = {
    let orders = self.pending_orders.read().unwrap();
    *orders.get(account).unwrap_or(&0)
}; // Lock released!
// RACE WINDOW!
let new_value = current + amount;
let mut orders = self.pending_orders.write().unwrap();
orders.insert(account, new_value); // May overwrite!

// FIXED: Hold lock for entire sequence
let mut orders = self.pending_orders.lock().unwrap();
let current = *orders.get(account).unwrap_or(&0);
let new_value = current + amount;
orders.insert(account, new_value); // Atomic!
```

### Approach 2: Atomic Operations (Best for counters)
```rust
// Use AtomicU64 for lock-free atomic increment
let counter = orders.get(account).unwrap();
counter.fetch_add(amount, Ordering::SeqCst); // Hardware atomic!
```

### Approach 3: Concurrent Data Structures
```rust
// Use DashMap or similar concurrent HashMap
let concurrent_map = DashMap::new();
concurrent_map.entry(account)
    .and_modify(|v| *v += amount)
    .or_insert(amount);
```

## Distributed System Relevance

This bug is **critical** for:
- **Blockchain transaction processing**: Sui, Ethereum, Bitcoin (UTXO tracking)
- **Banking systems**: Account balance updates
- **E-commerce**: Inventory management (double-selling)
- **Distributed counters**: View counts, likes, metrics
- **Resource allocation**: Connection pools, thread pools
- **Any system with concurrent updates to shared state**

**Real-world consequences**:
- **Double-spending attacks** in cryptocurrency
- **Inventory overselling** in e-commerce
- **Lost metrics** in monitoring systems
- **Financial discrepancies** in banking

## Tool Detection

- **lockbud**: Unlikely to detect (correct locks used, just wrong granularity)
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (no undefined behavior, just logic error)
- **ThreadSanitizer**: May detect data race if no locks used
- **Static analysis**: Could detect with atomicity analysis
- **Formal verification**: Tools like TLA+ would catch this

## Notes

- This is the classic **"lost update"** database concurrency problem
- Also known as **"write skew"** or **"atomicity violation"**
- Different from "dirty read" - both transactions commit, but one is lost
- **Professor's hint**: This is exactly the "lost update problem" pattern mentioned!
- The bug is **timing-dependent** - may not manifest every run
- More likely to manifest under high contention
- Run multiple times to see different amounts of lost updates
- This pattern appears in the Sui CSV data as an "atomic" category bug
