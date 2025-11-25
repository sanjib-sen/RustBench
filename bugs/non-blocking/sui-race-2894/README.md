# Sui Race #2894: API Environment Loading Race

## Bug Information
- **Source**: Sui Blockchain (Wallet Extension)
- **Issue**: https://github.com/MystenLabs/sui/issues/2894
- **Fix PR**: https://github.com/MystenLabs/sui/pull/2913
- **Type**: Non-blocking bug (Race Condition)
- **Category**: Initialization race / Redundant lazy loading

## Root Cause

The Sui wallet extension was loading the API environment configuration from storage on-demand whenever a component needed it, rather than loading once during application initialization. This caused:

1. **Redundant I/O**: Configuration loaded multiple times from storage
2. **Race condition**: Multiple components initializing concurrently would all trigger separate loads
3. **Inefficiency**: The saved environment was fetched repeatedly despite being static
4. **Potential inconsistency**: Different loads might return different values if storage changes

The fix quote from the issue: *"initialize the state from the storage or the default env and always use the value from state"*

**Pattern**: Lazy initialization race with redundant resource loading

## Bug Pattern (Abstracted)

```
Thread 1 (UI Component)        Thread 2 (API Component)
-----------------------        ------------------------
get_api_environment()
  check if loaded
  not loaded!                  get_api_environment()
  load from storage (50ms)       check if loaded
                                 not loaded!
                                 load from storage (50ms)
  cache result                   cache result

Result: Storage loaded 2x instead of 1x!
```

## Reproduction Steps

### Running the Buggy Version

```bash
cargo run
```

**Expected Output**:
```
=== Sui Issue #2894: API Environment Loading Race ===

Running BUGGY version (lazy loading with race)...

[BUGGY] UI initializing...
[BUGGY] Loading API environment...
[BUGGY] API initializing...
[BUGGY] Loading API environment...
[BUGGY] Wallet initializing...
[BUGGY] Loading API environment...
  [STORAGE] Loading API environment from disk (load #1)
  [STORAGE] Loading API environment from disk (load #2)
[BUGGY] Network initializing...
[BUGGY] Loading API environment...
  [STORAGE] Loading API environment from disk (load #3)
[BUGGY] UI got environment
[BUGGY] API got environment
[BUGGY] Wallet got environment
  [STORAGE] Loading API environment from disk (load #4)
[BUGGY] Storage initializing...
[BUGGY] Loading API environment...
[BUGGY] Network got environment
  [STORAGE] Loading API environment from disk (load #5)
[BUGGY] Storage got environment

=== Results ===
Total storage loads: 5

[BUG DEMONSTRATED]
Configuration was loaded 5 times instead of once!
Multiple threads raced to load the same configuration.
This causes:
  - Wasted I/O operations
  - Potential inconsistent state
  - Unnecessary resource usage
```

**What Happens**:
- Five components (UI, API, Wallet, Network, Storage) initialize concurrently
- Each checks if config is loaded, finds it's not
- All trigger separate storage loads
- Configuration loaded 5 times instead of once
- Wasted I/O and CPU resources

### Running the Fixed Version (Load at Init)

```bash
cargo run -- --fixed
```

**Expected Output**:
```
=== Sui Issue #2894: API Environment Loading Race ===

Running FIXED version (load at init)...

[FIXED] Initializing app with environment from storage...
  [STORAGE] Loading API environment from disk (load #1)

[FIXED] UI initializing...
[FIXED] Wallet initializing...
[FIXED] Network initializing...
[FIXED] API initializing...
[FIXED] Storage initializing...
[FIXED] UI got environment
[FIXED] API got environment
[FIXED] Wallet got environment
[FIXED] Network got environment
[FIXED] Storage got environment

=== Results ===
Total storage loads: 1

[FIXED]
Configuration loaded exactly once during app initialization.
All components reuse the cached value.
```

### Running the Fixed Version (Using std::sync::Once)

```bash
cargo run -- --once
```

**Expected Output**:
```
=== Sui Issue #2894: API Environment Loading Race ===

Running FIXED-ONCE version (std::sync::Once)...

[FIXED-ONCE] UI initializing...
[FIXED-ONCE] Loading API environment (one-time init)...
  [STORAGE] Loading API environment from disk (load #1)
[FIXED-ONCE] UI got environment
[FIXED-ONCE] API initializing...
[FIXED-ONCE] API got environment
[FIXED-ONCE] Wallet initializing...
[FIXED-ONCE] Network initializing...
[FIXED-ONCE] Storage initializing...
[FIXED-ONCE] Wallet got environment
[FIXED-ONCE] Network got environment
[FIXED-ONCE] Storage got environment

=== Results ===
Total storage loads: 1

[FIXED-ONCE]
std::sync::Once ensures exactly-once initialization.
First thread loads, others wait for completion.
```

## Fix Strategy

Two valid approaches:

### 1. Load at Initialization (Recommended for Sui)
```rust
// Load configuration once during app startup
pub fn new(storage: Arc<Storage>) -> Self {
    let env = storage.load_api_environment(); // Load once
    Self {
        current_env: Mutex::new(env), // Cache it
    }
}

pub fn get_api_environment(&self) -> Environment {
    self.current_env.lock().unwrap().clone() // Reuse cached
}
```

**Pros**: Simple, predictable, no lazy overhead
**Cons**: Pays initialization cost even if not needed

### 2. std::sync::Once (Lazy + Thread-safe)
```rust
pub fn get_api_environment(&self) -> Environment {
    self.init_once.call_once(|| {
        let env = self.storage.load_api_environment();
        *self.current_env.lock().unwrap() = Some(env);
    });
    self.current_env.lock().unwrap().clone().unwrap()
}
```

**Pros**: Lazy initialization, thread-safe, exactly-once guarantee
**Cons**: Slightly more complex

## Distributed System Relevance

This bug is critical for:
- **Configuration management systems**: Consul, etcd clients
- **Wallet applications**: MetaMask, Phantom, Sui Wallet
- **Service discovery**: Loading service endpoints
- **Feature flags**: LaunchDarkly, Split clients
- **Database connection pools**: Loading connection configs
- **API clients**: Initializing base URLs, auth tokens

The pattern appears whenever multiple components need shared configuration loaded from external storage.

## Tool Detection

- **lockbud**: Unlikely to detect (no double-lock pattern)
- **Rudra**: Unlikely to detect (not memory safety)
- **miri**: Unlikely to detect (correct Rust semantics, just inefficient)
- **Static analysis**: Could detect missing Once or eager initialization
- **Runtime profiling**: Would show redundant I/O operations

## Notes

- This is a **performance bug** that can become a correctness bug if loaded values differ
- The race is timing-dependent and may not always manifest
- Run multiple times to see different interleavings
- In production, this caused unnecessary network requests in Sui wallet
- The bug demonstrates the "double-checked locking" anti-pattern without proper barriers
