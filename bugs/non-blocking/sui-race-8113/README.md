# Sui Race #8113: Concurrent Build Directory Race

## Bug Information
- **Source**: Sui Blockchain
- **Issue**: https://github.com/MystenLabs/sui/issues/8113
- **Fix PR**: https://github.com/MystenLabs/sui/pull/8857
- **Type**: Non-blocking bug (Race Condition)
- **Category**: Filesystem race

## Root Cause

Multiple processes attempting to build the same Move package simultaneously race to
create and populate the same build output directory. The `sui move build` command
defaults to using the current directory for build artifacts. When parallel test
execution occurs, competing processes race to create the same directory, causing
"Directory not empty (os error 66)" failures.

**Pattern**: Concurrent filesystem operations without coordination

## Bug Pattern (Abstracted)

```
Thread 1                        Thread 2
--------                        --------
check if dir exists             check if dir exists
  -> not exists                   -> not exists
create dir                      create dir
  -> success                      -> ERROR: already exists!
write files...                  write files...
  -> FILE CONFLICT!               -> FILE CONFLICT!
```

## Expected Behavior

When running the buggy version:
- Some threads will fail with directory creation errors
- Some threads may overwrite each other's files
- File contents may be corrupted or incomplete

## Fix Strategy

Use a temporary directory for each build operation to isolate concurrent builds:
```rust
// Before (buggy): All builds use same directory
let build_dir = PathBuf::from("./build");

// After (fixed): Each build gets unique directory
let build_dir = tempfile::tempdir().unwrap();
```

## How to Run

```bash
# Run the buggy version (shows race condition)
cargo run

# Run with fixed version
cargo run -- --fixed
```

## Tool Detection

- **lockbud**: May not detect (filesystem race, not lock-based)
- **Rudra**: May not detect (not a memory safety issue)
- **miri**: May not detect (depends on fs operations)

## Distributed System Relevance

This bug is highly relevant to distributed build systems where:
- Multiple CI/CD jobs build simultaneously
- Parallel test execution compiles packages
- Microservices share build caches
- Kubernetes pods share persistent volumes
