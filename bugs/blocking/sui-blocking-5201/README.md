# Sui Blocking #5201: Bounded Queue Deadlock

## Bug Information
- **Source**: Sui Blockchain (Certificate Waiter)
- **Issue**: https://github.com/MystenLabs/sui/issues/5201
- **Type**: Blocking bug (Deadlock)
- **Category**: Bounded channel/queue saturation

## Root Cause

The `CertificateWaiter` maintains a bounded wait queue (max 1000 items) for futures
awaiting parent certificate availability. When an authority receives certificates
missing parent certificates locally, it attempts to fetch those parents while
processing. This creates a cascading failure:

1. Certificate A arrives, needs parent P (not available)
2. A is queued waiting for P
3. Request to fetch P triggers more certificate processing
4. More certificates queue up waiting for parents
5. Queue reaches capacity (1000)
6. New certificates can't be queued → never processed
7. System deadlocks: blocked certificates can't make progress

**Pattern**: Bounded producer-consumer with recursive dependency

## Bug Pattern (Abstracted)

```
Producer Thread                 Consumer/Worker
--------------                  ---------------
receive_cert(A)
  needs_parent(P)
  queue.push(wait_for(P))       <-- queue filling up
  fetch_parent(P)
    receive_cert(P')
      needs_parent(Q)
      queue.push(wait_for(Q))   <-- queue filling more
      ...
                                queue.pop() -> process
                                (too slow!)
queue FULL!
  receive_cert(X)
  queue.push(wait) -> BLOCKED!  <-- DEADLOCK
```

## Expected Behavior

When running the buggy version:
- Producer thread blocks when trying to push to full queue
- System deadlocks if consumer can't drain queue fast enough
- In distributed systems: entire node stops processing

## Fix Strategy

1. Use unbounded queue with backpressure (not blocking)
2. Process certificates in topological order
3. Use disk-based temporary storage for overflow
4. Implement work-stealing or priority queuing

## How to Run

```bash
# Run the buggy version (shows deadlock)
cargo run

# Run with fixed version (unbounded/backpressure)
cargo run -- --fixed
```

## Tool Detection

- **lockbud**: May not detect (not traditional lock deadlock)
- **Rudra**: May not detect (not memory safety)
- **miri**: May not detect (bounded channel semantics)

## Distributed System Relevance

This bug is highly relevant to:
- Blockchain consensus protocols
- Message queue systems (RabbitMQ, Kafka consumers)
- Work queue processing (job schedulers)
- Dependency resolution systems (package managers)
- Any system with bounded buffers and recursive processing
