# Reth Deadlock #10842: Lock Ordering Deadlock

## Bug Information
- **Source**: Reth (Rust Ethereum)
- **Fix PR**: https://github.com/paradigmxyz/reth/pull/10842
- **Type**: Blocking bug (Deadlock)
- **Category**: Lock ordering deadlock / Circular wait

## Root Cause

The chain state module used two RwLocks to protect in-memory state:
- **numbers lock**: Maps block hashes to block numbers
- **blocks lock**: Maps block numbers to block data

Different operations acquired these locks in **inconsistent order**:
- **Read operations**: `numbers` → `blocks`
- **Write operations**: `blocks` → `numbers` (WRONG ORDER!)

This creates a circular wait condition leading to deadlock.

**Pattern**: Classic lock ordering deadlock (circular dependency)

## Bug Pattern (Abstracted)

```
Thread 1 (Read)              Thread 2 (Write)
---------------              ----------------
acquire numbers(read)
                             acquire blocks(write)
  waiting for blocks(read)
                               waiting for numbers(write)

DEADLOCK! Circular wait.
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== Reth Issue #10842: Lock Ordering Deadlock ===

Running BUGGY version (inconsistent lock order)...

NOTE: This may deadlock! Kill with Ctrl+C if it hangs.

Starting two threads with conflicting lock orders...

[Thread 1] Starting read_operation
[BUGGY] read_operation: acquiring numbers lock...
[Thread 2] Starting write_operation
[BUGGY] write_operation: acquiring blocks lock...
[BUGGY] read_operation: acquiring blocks lock...
[BUGGY] write_operation: acquiring numbers lock...

Waiting for threads to complete (may deadlock)...

=== Results ===
[DEADLOCK DETECTED]
Threads did not complete within 3 seconds!

Deadlock scenario:
  Thread 1: holds numbers(read), waiting for blocks(read)
  Thread 2: holds blocks(write), waiting for numbers(write)

Classic lock ordering deadlock!

Run with --fixed to see consistent lock ordering.
```

**What Happens**:
1. Thread 1 acquires `numbers` read lock
2. Thread 2 acquires `blocks` write lock
3. Thread 1 tries to acquire `blocks` read lock → blocked (write lock held)
4. Thread 2 tries to acquire `numbers` write lock → blocked (read lock held)
5. **Circular wait → DEADLOCK**

### Running the Fixed Version

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Reth Issue #10842: Lock Ordering Deadlock ===

Running FIXED version (consistent lock order)...

Starting two threads with consistent lock order...

[Thread 1] Starting read_operation
[FIXED] read_operation: acquiring numbers lock...
[FIXED] read_operation: acquiring blocks lock...
[Thread 1] Completed read_operation
[Thread 2] Starting write_operation
[FIXED] write_operation: acquiring numbers lock first...
[FIXED] write_operation: acquiring blocks lock...
[FIXED] write_operation: completed
[Thread 2] Completed write_operation

=== Results ===
[FIXED]
Both threads completed successfully!
Consistent lock order (numbers -> blocks) prevents deadlock.
```

**What Happens**:
- All operations acquire locks in the same order: `numbers` → `blocks`
- No circular wait possible
- Operations complete successfully

## Fix Strategy

### BUGGY: Inconsistent Lock Order
```rust
// Read operation: numbers → blocks
fn read_operation(&self, hash: &str) -> Option<Block> {
    let numbers = self.state.numbers.read().unwrap();
    let blocks = self.state.blocks.read().unwrap();
    // ...
}

// Write operation: blocks → numbers (WRONG!)
fn write_operation(&self, block: Block) {
    let mut blocks = self.state.blocks.write().unwrap();
    let mut numbers = self.state.numbers.write().unwrap(); // DEADLOCK!
    // ...
}
```

### FIXED: Consistent Lock Order
```rust
// RULE: Always acquire numbers lock before blocks lock

// Read operation: numbers → blocks
fn read_operation(&self, hash: &str) -> Option<Block> {
    let numbers = self.state.numbers.read().unwrap();
    let blocks = self.state.blocks.read().unwrap();
    // ...
}

// Write operation: numbers → blocks (CONSISTENT!)
fn write_operation(&self, block: Block) {
    let mut numbers = self.state.numbers.write().unwrap();
    let mut blocks = self.state.blocks.write().unwrap();
    // ...
}
```

### Lock Ordering Discipline

**Documentation added** (from PR #10842):
```rust
/// IMPORTANT: Lock ordering discipline
/// Always acquire locks in this order:
/// 1. numbers lock
/// 2. blocks lock
/// Violating this order can cause deadlock!
```

## Distributed System Relevance

This bug is critical for:
- **Blockchain implementations**: Ethereum, Bitcoin (chain state management)
- **Databases**: Transaction managers with multiple locks
- **File systems**: inode and data block locks
- **Operating systems**: Process and resource locks
- **Distributed caches**: Multi-level cache hierarchies
- **Any system with multiple related locks**

**Real-world impact in Reth**:
- Users experienced node freezes
- Required node restarts
- Fixed in v1.0.7 release

## Tool Detection

- **lockbud**: May detect lock ordering violations
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (correct Rust semantics, just wrong order)
- **ThreadSanitizer**: Can detect lock order inversions
- **Static analysis**: Lock order checkers can detect this
- **Formal verification**: Model checkers (Spin, TLA+) would detect

## Notes

- This is the **classic dining philosophers** deadlock pattern
- Lock ordering is a fundamental principle in concurrent programming
- **Solution**: Establish a global lock ordering and enforce it everywhere
- The fix is simple but requires careful code review
- This type of bug is timing-dependent and hard to reproduce
- Can take hours/days to manifest in production
- Best prevented by: code review, static analysis, and documentation
