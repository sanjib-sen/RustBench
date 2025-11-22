# SurrealDB Deadlock #3987: RwLock Contention Deadlock

## Bug Information
- **Source**: SurrealDB Database
- **Issue**: https://github.com/surrealdb/surrealdb/issues/3987
- **Type**: Blocking bug (Deadlock)
- **Category**: RwLock contention under high load

## Root Cause

The deadlock occurs in `src/net/rpc.rs` when checking WebSocket connections. A global
`RwLock<HashMap>` protects WebSocket connection metadata. Under high contention with
live queries and multiple client connections, the blocking `.read().await` calls can
deadlock when:

1. Multiple readers continuously hold read locks
2. A writer needs to acquire write lock (blocked by readers)
3. New readers can't proceed (blocked by waiting writer)
4. System reaches deadlock state

**Pattern**: RwLock writer starvation / priority inversion

## Bug Pattern (Abstracted)

```rust
// BUGGY: Blocking read under contention
async fn check_connection(id: u64) -> bool {
    // This .read().await can deadlock under high contention
    let guard = CONNECTIONS.read().await;
    guard.contains_key(&id)
}

// FIXED: Non-blocking with retry
async fn check_connection(id: u64) -> bool {
    loop {
        match CONNECTIONS.try_read() {
            Ok(guard) => return guard.contains_key(&id),
            Err(_) => tokio::time::sleep(Duration::from_millis(1)).await,
        }
    }
}
```

## Expected Behavior

When running the buggy version:
- System eventually deadlocks under high load
- All tasks waiting on RwLock
- No progress made

## Fix Strategy

1. Use `try_read()` with backoff instead of blocking `.read().await`
2. Consider using `parking_lot::RwLock` with better fairness
3. Reduce lock scope and duration
4. Use lock-free data structures where possible

## How to Run

```bash
# Run the buggy version (may deadlock)
cargo run

# Run with fixed version (try_read with backoff)
cargo run -- --fixed
```

## Tool Detection

- **lockbud**: May detect (RwLock pattern analysis)
- **Rudra**: May not detect (not memory safety)
- **tokio-console**: Useful for runtime debugging

## Distributed System Relevance

This bug is highly relevant to:
- WebSocket servers with many concurrent connections
- Real-time notification systems (live queries)
- Chat servers and presence systems
- Any service with shared connection registries
