# Sui Blocking #828: Sync Mutex in Async Context

## Bug Information
- **Source**: Sui Blockchain REST Server
- **Issue**: https://github.com/MystenLabs/sui/issues/828
- **Fix PR**: https://github.com/MystenLabs/sui/pull/817
- **Type**: Blocking bug
- **Category**: Async runtime blocking

## Root Cause

The REST server used `std::sync::Mutex` (synchronous mutex) to protect shared state
in an async context. This caused the async runtime to block when holding the mutex
across `.await` points, serializing concurrent requests and causing timeouts.

The developer noted: "the problem was Mutex, the mutex we use before was `sync::Mutex`
so it doesn't impl Send and that's why it won't compile"

**Fix**: Change from `std::sync::Mutex` to `futures::lock::Mutex` (or `tokio::sync::Mutex`)

## Bug Pattern (Abstracted)

```rust
// BUGGY: std::sync::Mutex in async code
async fn handle_request(state: Arc<std::sync::Mutex<State>>) {
    let guard = state.lock().unwrap();  // Blocks entire thread!
    do_async_work().await;              // Other tasks can't run
    drop(guard);
}

// FIXED: Use async-aware mutex
async fn handle_request(state: Arc<tokio::sync::Mutex<State>>) {
    let guard = state.lock().await;     // Yields to runtime
    do_async_work().await;              // Other tasks can run
    drop(guard);
}
```

## Expected Behavior

When running the buggy version:
- Concurrent async tasks are serialized (run one at a time)
- Overall throughput degrades significantly
- May cause deadlocks if tasks depend on each other

## How to Run

```bash
# Run the buggy version (sync mutex blocks async runtime)
cargo run

# Run with fixed version (async mutex)
cargo run -- --fixed
```

## Tool Detection

- **lockbud**: May detect (blocking in async context)
- **Rudra**: May not detect (not memory safety)
- **clippy**: Has lint `await_holding_lock` for this pattern

## Distributed System Relevance

This bug is critical for:
- Async web servers (actix, axum, warp)
- gRPC services (tonic)
- Database connection pools
- Any distributed service handling concurrent requests
- Microservices with shared state
