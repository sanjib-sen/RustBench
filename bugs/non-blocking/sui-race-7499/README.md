# Sui Race #7499: Port Binding Race Condition

## Bug Information
- **Source**: Sui Blockchain (Narwhal worker)
- **Issue**: https://github.com/MystenLabs/sui/issues/7499
- **Fix PR**: https://github.com/MystenLabs/sui/pull/7509
- **Type**: Non-blocking bug (Race Condition / TOCTOU)
- **Category**: Network resource race

## Root Cause

When `TxReceiverHandler::spawn` attempts to bind to a network address, it can fail
with "Address already in use" even though the port is about to be released by another
process. The timing gap (as short as 9 microseconds in observed cases) between port
release and re-bind attempt creates a race condition.

**Pattern**: Time-of-Check to Time-of-Use (TOCTOU) in network initialization

The problematic code used `.unwrap()` on bind, causing immediate panic on transient
binding failures rather than handling the temporary error.

## Bug Pattern (Abstracted)

```
Thread 1 (Server A)              Thread 2 (Server B)
------------------               ------------------
bind(port 4000) -> OK
listening...
                                 wants to start on 4000
close(port 4000)                 bind(port 4000) -> ERROR!
  |                              (Address already in use)
  v
port 4000 now free               panic! (9 microseconds too early)
```

## Expected Behavior

When running the buggy version:
- Thread 2 may panic with "Address already in use" error
- The timing is non-deterministic, may succeed sometimes
- In production: service restart failures, orchestration issues

## Fix Strategy

Implement retry logic with backoff instead of immediate panic:

```rust
// Before (buggy): Immediate panic on bind failure
let listener = TcpListener::bind(addr).unwrap();

// After (fixed): Retry with backoff
let listener = retry_with_backoff(|| TcpListener::bind(addr), max_retries, delay)?;
```

## How to Run

```bash
# Run the buggy version (may show bind failure)
cargo run

# Run with fixed version (retry logic)
cargo run -- --fixed
```

## Tool Detection

- **lockbud**: May not detect (not lock-related)
- **Rudra**: May not detect (not memory safety)
- **miri**: May not detect (OS-level timing)

## Distributed System Relevance

This bug is highly relevant to:
- Microservices with dynamic port allocation
- Container orchestration (Kubernetes pod restarts)
- Load balancers and service mesh proxies
- Database connection pooling
- Any system with rapid service restart cycles
