# SurrealDB Race #5042: Concurrent Authentication Race

## Bug Information
- **Source**: SurrealDB Database
- **Issue**: https://github.com/surrealdb/surrealdb/issues/5042
- **Fix PR**: https://github.com/surrealdb/surrealdb/pull/5130
- **Type**: Non-blocking bug (Race Condition)
- **Category**: Concurrent write conflict / Authentication race

## Root Cause

When multiple concurrent HTTP requests authenticate with the same Bearer token, they all execute `UPDATE $auth SET lastActive = time::now()` simultaneously on the same authentication record. This creates a write-write conflict where:

1. Multiple threads validate the token (read)
2. All proceed to update `lastActive` (write)
3. Concurrent updates conflict
4. Only the first succeeds, others fail with "There was a problem with authentication"

This is a classic **write-write conflict** or **lost update** problem in the authentication path.

**Pattern**: Non-atomic read-then-update causing concurrent write conflicts

## Bug Pattern (Abstracted)

```
Request 1                    Request 2                    Request 3
---------                    ---------                    ---------
Read token (valid)           Read token (valid)           Read token (valid)
  lastActive = T1              lastActive = T1              lastActive = T1

Validate OK                  Validate OK                  Validate OK

UPDATE lastActive = T2       UPDATE lastActive = T3       UPDATE lastActive = T4
  SUCCESS!                     CONFLICT!                    CONFLICT!
                               Auth FAILED!                 Auth FAILED!

Result: Only 1 out of 3 requests succeed!
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== SurrealDB Issue #5042: Concurrent Authentication Race ===

Running BUGGY version (racy read-then-update)...

Simulating 10 concurrent authentication requests...

[BUGGY] Request 0 authenticating...
[BUGGY] Request 1 authenticating...
[BUGGY] Request 2 authenticating...
[BUGGY] Authentication SUCCESS for token 'token_123' by user 'alice'
[BUGGY] Request 3 authenticating...
[BUGGY] Request 4 authenticating...
[BUGGY] Authentication FAILED for token 'token_123' - concurrent update detected
[BUGGY] Authentication FAILED for token 'token_123' - concurrent update detected
[BUGGY] Request 5 authenticating...
[BUGGY] Request 6 authenticating...
[BUGGY] Authentication FAILED for token 'token_123' - concurrent update detected
...

=== Results ===
Successful authentications: 3
Failed authentications: 7

[BUG DEMONSTRATED]
Only 3 out of 10 requests succeeded!
Concurrent UPDATE $auth SET lastActive caused race conflicts.
In production, this caused 'There was a problem with authentication' errors.
```

**What Happens**:
- 10 concurrent requests with the same valid Bearer token
- All pass initial validation (token exists)
- They race to update `lastActive`
- Only 1-3 succeed, rest fail with authentication errors
- **Result**: 70% authentication failure rate!

**Production Impact**:
- Users with valid tokens get spurious auth errors
- High concurrency makes the bug worse
- Affects `/sql` HTTP endpoint
- Reported in version 2.0.4+20241025

### Running the Fixed Version

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== SurrealDB Issue #5042: Concurrent Authentication Race ===

Running FIXED version (atomic validate-and-update)...

Simulating 10 concurrent authentication requests...

[FIXED] Request 0 authenticating...
[FIXED] Authentication SUCCESS for token 'token_123' by user 'alice'
[FIXED] Request 1 authenticating...
[FIXED] Authentication SUCCESS for token 'token_123' by user 'alice'
[FIXED] Request 2 authenticating...
[FIXED] Authentication SUCCESS for token 'token_123' by user 'alice'
...

=== Results ===
Successful authentications: 10
Failed authentications: 0

[FIXED]
All 10 requests succeeded!
Atomic validate-and-update prevents race condition.
Write lock held during entire authentication sequence.
```

**What Happens**:
- Write lock held during entire validate-and-update sequence
- No race window between read and write
- All 10 requests succeed
- **Result**: 100% success rate!

## Fix Strategy

### BUGGY: Separate Read and Write
```rust
// Step 1: Read (with read lock)
let token_data = {
    let tokens = self.store.tokens.read().unwrap();
    tokens.get(token).cloned()
}; // Lock released!

// RACE WINDOW HERE!

// Step 2: Write (with write lock)
let mut tokens = self.store.tokens.write().unwrap();
tokens.get_mut(token).unwrap().last_active = now();
// Conflict with other concurrent updates!
```

### FIXED: Atomic Validate-and-Update
```rust
// Hold write lock for entire sequence
let mut tokens = self.store.tokens.write().unwrap();

match tokens.get_mut(token) {
    Some(auth_token) => {
        // Atomically validate and update
        auth_token.last_active = SystemTime::now();
        AuthResult::Success
    }
    None => AuthResult::Failed,
}
// Lock held until completion - no race!
```

### Alternative Fix: Conditional Update
```rust
// Use compare-and-swap or version numbers
UPDATE $auth
SET lastActive = time::now()
WHERE id = $token_id
  AND lastActive = $expected_time
```

## Distributed System Relevance

This bug is critical for:
- **Authentication systems**: OAuth servers, JWT validation
- **Session management**: Active session tracking
- **Rate limiting**: Last-access timestamp updates
- **API gateways**: Concurrent request authentication
- **Distributed caches**: Hot key updates (Redis, Memcached)
- **Load balancers**: Health check timestamp updates
- **Monitoring systems**: Metric timestamp updates

**Real-world consequences**:
- Spurious authentication failures for valid users
- Poor user experience (random auth errors)
- Difficult to debug (timing-dependent)
- Worse under high load (more concurrency = more failures)

## Tool Detection

- **lockbud**: Unlikely to detect (locks used correctly, just wrong granularity)
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (no undefined behavior)
- **ThreadSanitizer**: May detect data race if no locks used
- **Static analysis**: Could detect non-atomic read-modify-write patterns

## Notes

- This is a **write-write conflict** bug (also called "lost update")
- Similar to sui-race-303 (both are atomicity violations)
- **Timing-dependent**: More likely under high concurrency
- The bug demonstrates why authentication paths need careful synchronization
- PR #5130 improved error handling but the core fix is atomic updates
- Related to issue #5114 which identified the concurrent UPDATE as the root cause
