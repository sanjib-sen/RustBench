# Sui Blocking #5204: BoundedExecutor Head-of-Line Blocking

## Bug Information
- **Source**: Sui Blockchain / Narwhal Consensus
- **Issue**: https://github.com/MystenLabs/sui/issues/5204
- **Related**: narwhal#559, narwhal#724
- **Type**: Blocking bug (Head-of-Line Blocking)
- **Category**: Executor exhaustion / Bounded resource starvation

## Root Cause

An architectural change (narwhal#559) introduced "one bounded executor per destination" rather than a single global executor. When any executor exhausts its available tickets (concurrent task slots), the `send_message` function becomes **blocking**.

**The problem**: When one validator's executor fills up, shared code paths become blocked for **all** validators attempting to use that path.

**Pattern**: Head-of-line blocking where one slow consumer starves all producers

## Bug Pattern (Abstracted)

```
Sender 1 → Executor (capacity: 3) → Slow Validator A
Sender 2 → [Slot 1: processing...]
Sender 3 → [Slot 2: processing...]
Sender 4 → [Slot 3: processing...]
Sender 5 → BLOCKED! (waiting for slot)
Sender 6 → BLOCKED! (waiting for slot)
...       → BLOCKED!

Result: ALL senders blocked because ONE validator is slow!
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== Sui Issue #5204: BoundedExecutor Head-of-Line Blocking ===

Running BUGGY version (blocking send)...

Executor capacity: 3
Sending 10 messages with slow processing...

[BUGGY] Sender 0 attempting to send at 4.25µs...
[BLOCKING] Message from 'sender_0' to 'validator_0' queued (may have blocked)
[BUGGY] Sender 1 attempting to send at 10.288042ms...
[BLOCKING] Message from 'sender_1' to 'validator_1' queued (may have blocked)
[BUGGY] Sender 2 attempting to send at 20.495ms...
[BLOCKING] Message from 'sender_2' to 'validator_2' queued (may have blocked)
[BUGGY] Sender 3 attempting to send at 30.773667ms...
[EXECUTOR] Processed message from 'sender_0' to 'validator_0'
[BLOCKING] Message from 'sender_3' to 'validator_0' queued (may have blocked)
[BUGGY] Sender 4 attempting to send at 41.049084ms...
[BUGGY] Sender 4 BLOCKED for 90.016708ms! (Head-of-line blocking)
[BLOCKING] Message from 'sender_4' to 'validator_1' queued (may have blocked)
...

=== Results ===
Multiple senders were blocked waiting for executor capacity.

[BUG DEMONSTRATED]
When one executor runs out of tickets, ALL senders block!
This is head-of-line blocking - slow validator starves others.
In Sui, this caused 'tx_helper_requests' occupancy to spike.
```

**What Happens**:
1. First 3 messages fill the executor (capacity = 3)
2. Executor processes slowly (100ms per message)
3. Subsequent senders block waiting for capacity
4. Sender 4+ blocked for 90+ milliseconds
5. **All producers starved by one slow consumer**

### Running the Fixed Version

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui Issue #5204: BoundedExecutor Head-of-Line Blocking ===

Running FIXED version (non-blocking with drop policy)...

Executor capacity: 3
Sending 10 messages with slow processing...

[FIXED] Sender 0 attempting to send at 4.541µs...
[NONBLOCKING] Message from 'sender_0' to 'validator_0' queued
[FIXED] Sender 0 completed immediately (no blocking)
[FIXED] Sender 1 attempting to send at 10.267875ms...
[NONBLOCKING] Message from 'sender_1' to 'validator_1' queued
[FIXED] Sender 1 completed immediately (no blocking)
[FIXED] Sender 2 attempting to send at 20.487125ms...
[NONBLOCKING] Message from 'sender_2' to 'validator_2' queued
[FIXED] Sender 2 completed immediately (no blocking)
[FIXED] Sender 3 attempting to send at 30.756417ms...
[NONBLOCKING] Message from 'sender_3' to 'validator_0' DROPPED (executor full)
[FIXED] Sender 3 completed immediately (no blocking)
...

=== Results ===
Dropped 7 messages when executor was full

[FIXED]
Non-blocking send with drop policy prevents head-of-line blocking.
Senders to overloaded validators don't block other senders.
Messages are dropped instead of blocking the entire system.
```

**What Happens**:
1. First 3 messages fill the executor
2. Subsequent sends use `try_send` (non-blocking)
3. Messages 4-10 are immediately dropped
4. **No senders blocked!**
5. System remains responsive

## Fix Strategy

### For UnreliableNetwork
```rust
// BUGGY: Blocking send
pub fn send_message(&self, msg: Message) -> Result<(), Error> {
    self.executor.send(msg)?; // Blocks if full!
    Ok(())
}

// FIXED: Non-blocking with drop
pub fn send_message(&self, msg: Message) -> Result<(), Error> {
    match self.executor.try_send(msg) {
        Ok(_) => Ok(()),
        Err(TrySendError::Full(_)) => {
            // Drop message - acceptable for unreliable network
            Err(Error::ExecutorFull)
        }
        Err(e) => Err(e.into()),
    }
}
```

**Rationale**: For unreliable networks (like helper requests), messages aren't critical for protocol progress. Dropping them is better than blocking.

### For ReliableNetwork
```rust
// FIXED: Pre-acquire capacity before entering critical section
pub async fn send_message(&self, msg: Message) -> Result<(), Error> {
    // Acquire permit BEFORE entering tokio::select
    let permit = self.executor.acquire_permit().await;

    // Now we're guaranteed not to block
    tokio::select! {
        // Can safely send without blocking
        _ = self.executor.spawn_with_permit(permit, process(msg)) => {},
        // Other branches...
    }
}
```

**Rationale**: For reliable networks, pre-acquire sending capacity to ensure non-blocking execution paths.

## Distributed System Relevance

This bug is critical for:
- **Consensus protocols**: Narwhal, HotStuff, PBFT (validator message passing)
- **P2P networks**: BitTorrent, IPFS (peer communication)
- **Microservices**: gRPC, HTTP/2 (service-to-service calls)
- **Message brokers**: Kafka, RabbitMQ (producer slowdown)
- **Load balancers**: One slow backend blocks all connections
- **Thread pools**: One slow task blocks all submitters

**Real-world impact in Sui**:
- Sharp increase in `tx_helper_requests` occupancy
- Network starvation across validators
- Cascading delays in consensus protocol

## Tool Detection

- **lockbud**: Unlikely to detect (no explicit lock deadlock)
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (correct channel semantics, just poor design)
- **Static analysis**: Could detect blocking calls in async contexts
- **Runtime profiling**: Would show thread blocking patterns

## Notes

- This is a **performance/liveness bug** rather than correctness
- Classic **head-of-line blocking** problem from networking
- Similar to HTTP/1.1 pipelining issues (solved by HTTP/2 multiplexing)
- The bug demonstrates why **backpressure** strategies matter
- Bounded executors are good for **resource management** but can cause **starvation**
- Trade-off: Dropping messages (availability) vs blocking (consistency)
- The fix depends on network semantics (reliable vs unreliable)
