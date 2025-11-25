//! GreptimeDB PR #3771: Region Guard Not Dropped on Procedure Completion
//!
//! This reproduces a data race where the guard protecting a dropping region
//! is not released when the procedure completes, allowing subsequent operations
//! to see inconsistent state.
//!
//! Original PR: https://github.com/GreptimeTeam/greptimedb/pull/3771

use std::collections::HashSet;
use std::env;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

pub type RegionId = u64;

/// Tracks which regions are currently being operated on
pub struct OperatingRegions {
    dropping: RwLock<HashSet<RegionId>>,
    creating: RwLock<HashSet<RegionId>>,
}

impl OperatingRegions {
    fn new() -> Self {
        Self {
            dropping: RwLock::new(HashSet::new()),
            creating: RwLock::new(HashSet::new()),
        }
    }

    fn is_dropping(&self, region_id: RegionId) -> bool {
        self.dropping.read().unwrap().contains(&region_id)
    }

    fn mark_dropping(&self, region_id: RegionId) {
        self.dropping.write().unwrap().insert(region_id);
    }

    fn unmark_dropping(&self, region_id: RegionId) {
        self.dropping.write().unwrap().remove(&region_id);
    }
}

/// Guard that should mark region as not-dropping when dropped
pub struct DroppingRegionGuard {
    region_id: RegionId,
    operating_regions: Arc<OperatingRegions>,
    released: bool,
}

impl DroppingRegionGuard {
    fn new(region_id: RegionId, operating_regions: Arc<OperatingRegions>) -> Self {
        operating_regions.mark_dropping(region_id);
        Self {
            region_id,
            operating_regions,
            released: false,
        }
    }

    fn release(&mut self) {
        if !self.released {
            self.operating_regions.unmark_dropping(self.region_id);
            self.released = true;
        }
    }
}

impl Drop for DroppingRegionGuard {
    fn drop(&mut self) {
        // Only unmark if not explicitly released
        if !self.released {
            self.operating_regions.unmark_dropping(self.region_id);
        }
    }
}

/// Region data
pub struct Region {
    id: RegionId,
    data: Vec<u8>,
    dropped: bool,
}

/// Region storage
pub struct RegionStore {
    regions: Mutex<Vec<Region>>,
}

impl RegionStore {
    fn new() -> Self {
        Self {
            regions: Mutex::new(vec![
                Region { id: 1, data: vec![1, 2, 3], dropped: false },
                Region { id: 2, data: vec![4, 5, 6], dropped: false },
            ]),
        }
    }

    fn drop_region(&self, region_id: RegionId) {
        let mut regions = self.regions.lock().unwrap();
        if let Some(region) = regions.iter_mut().find(|r| r.id == region_id) {
            region.dropped = true;
            region.data.clear();
        }
    }

    fn is_region_valid(&self, region_id: RegionId) -> bool {
        let regions = self.regions.lock().unwrap();
        regions.iter().any(|r| r.id == region_id && !r.dropped)
    }

    fn read_region(&self, region_id: RegionId) -> Option<Vec<u8>> {
        let regions = self.regions.lock().unwrap();
        regions.iter()
            .find(|r| r.id == region_id && !r.dropped)
            .map(|r| r.data.clone())
    }
}

/// Buggy version - guard not released when procedure completes
mod buggy {
    use super::*;

    pub struct DropTableProcedure {
        operating_regions: Arc<OperatingRegions>,
        store: Arc<RegionStore>,
    }

    impl DropTableProcedure {
        pub fn new(operating_regions: Arc<OperatingRegions>, store: Arc<RegionStore>) -> Self {
            Self { operating_regions, store }
        }

        /// BUG: Returns before guard is properly released
        pub fn execute(&self, region_id: RegionId) -> bool {
            println!("[BUGGY] Starting drop procedure for region {}", region_id);

            // Create guard - marks region as dropping
            let guard = DroppingRegionGuard::new(region_id, Arc::clone(&self.operating_regions));
            println!("[BUGGY] Region {} marked as dropping", region_id);

            // Simulate procedure execution
            thread::sleep(Duration::from_millis(50));

            // Actually drop the region
            self.store.drop_region(region_id);
            println!("[BUGGY] Region {} data dropped", region_id);

            // BUG: Procedure returns, but guard is stored/leaked elsewhere
            // In the real bug, the guard was held by a different component
            // that didn't release it when the procedure completed.
            std::mem::forget(guard); // Simulates guard not being dropped!

            println!("[BUGGY] Procedure returned (guard NOT released!)");
            true
        }
    }

    /// Reader that checks operating regions before accessing
    pub struct RegionReader {
        operating_regions: Arc<OperatingRegions>,
        store: Arc<RegionStore>,
    }

    impl RegionReader {
        pub fn new(operating_regions: Arc<OperatingRegions>, store: Arc<RegionStore>) -> Self {
            Self { operating_regions, store }
        }

        pub fn read(&self, region_id: RegionId) -> Result<Vec<u8>, &'static str> {
            // Check if region is being dropped
            if self.operating_regions.is_dropping(region_id) {
                println!("[BUGGY] Reader: region {} still marked as dropping!", region_id);
                return Err("region is being dropped");
            }

            // Try to read - but region was already dropped!
            if let Some(data) = self.store.read_region(region_id) {
                Ok(data)
            } else {
                println!("[BUGGY] Reader: region {} already dropped but not marked!", region_id);
                Err("region not found (inconsistent state!)")
            }
        }
    }
}

/// Fixed version - guard properly released when procedure completes
mod fixed {
    use super::*;

    pub struct DropTableProcedure {
        operating_regions: Arc<OperatingRegions>,
        store: Arc<RegionStore>,
    }

    impl DropTableProcedure {
        pub fn new(operating_regions: Arc<OperatingRegions>, store: Arc<RegionStore>) -> Self {
            Self { operating_regions, store }
        }

        /// FIX: Guard is properly released before procedure returns
        pub fn execute(&self, region_id: RegionId) -> bool {
            println!("[FIXED] Starting drop procedure for region {}", region_id);

            // Create guard
            let mut guard = DroppingRegionGuard::new(region_id, Arc::clone(&self.operating_regions));
            println!("[FIXED] Region {} marked as dropping", region_id);

            thread::sleep(Duration::from_millis(50));

            self.store.drop_region(region_id);
            println!("[FIXED] Region {} data dropped", region_id);

            // FIX: Explicitly release the guard before returning
            guard.release();
            println!("[FIXED] Guard released, region no longer marked as dropping");

            true
        }
    }

    pub struct RegionReader {
        operating_regions: Arc<OperatingRegions>,
        store: Arc<RegionStore>,
    }

    impl RegionReader {
        pub fn new(operating_regions: Arc<OperatingRegions>, store: Arc<RegionStore>) -> Self {
            Self { operating_regions, store }
        }

        pub fn read(&self, region_id: RegionId) -> Result<Vec<u8>, &'static str> {
            if self.operating_regions.is_dropping(region_id) {
                return Err("region is being dropped");
            }

            if let Some(data) = self.store.read_region(region_id) {
                Ok(data)
            } else {
                // This is expected after a completed drop
                Err("region not found (correctly dropped)")
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let use_fixed = args.iter().any(|arg| arg == "--fixed");

    println!("=== GreptimeDB PR #3771: Region Guard Not Released ===\n");

    if use_fixed {
        println!("Running FIXED version (guard released on completion)...\n");
        run_fixed_test();
    } else {
        println!("Running BUGGY version (guard leaked)...\n");
        run_buggy_test();
    }
}

fn run_buggy_test() {
    let operating_regions = Arc::new(OperatingRegions::new());
    let store = Arc::new(RegionStore::new());

    let region_id = 1;

    println!("Scenario: Drop table procedure followed by region check\n");

    // Drop procedure
    let procedure = buggy::DropTableProcedure::new(
        Arc::clone(&operating_regions),
        Arc::clone(&store),
    );
    procedure.execute(region_id);

    println!();

    // Try to read - will see inconsistent state!
    let reader = buggy::RegionReader::new(
        Arc::clone(&operating_regions),
        Arc::clone(&store),
    );

    println!("Checking region state after procedure completed...");
    let is_dropping = operating_regions.is_dropping(region_id);
    let is_valid = store.is_region_valid(region_id);

    println!("\n=== Results ===");
    println!("Region {} still marked as dropping: {}", region_id, is_dropping);
    println!("Region {} actually valid: {}", region_id, is_valid);

    if is_dropping && !is_valid {
        println!("\n[BUG DEMONSTRATED]");
        println!("Inconsistent state!");
        println!("  - Guard says region is being dropped (still held)");
        println!("  - But region was already dropped!");
        println!("  - New operations will be blocked forever");
        println!("\nThis causes data races when:");
        println!("  1. New table creation with same region ID blocked");
        println!("  2. Queries see 'dropping' state indefinitely");
        println!("\nRun with --fixed to see proper guard release.");
    }

    // Demonstrate the reader behavior
    match reader.read(region_id) {
        Ok(_) => println!("\nUnexpected: read succeeded"),
        Err(e) => println!("\nReader error: {}", e),
    }
}

fn run_fixed_test() {
    let operating_regions = Arc::new(OperatingRegions::new());
    let store = Arc::new(RegionStore::new());

    let region_id = 1;

    println!("Scenario: Drop table procedure followed by region check\n");

    let procedure = fixed::DropTableProcedure::new(
        Arc::clone(&operating_regions),
        Arc::clone(&store),
    );
    procedure.execute(region_id);

    println!();

    let reader = fixed::RegionReader::new(
        Arc::clone(&operating_regions),
        Arc::clone(&store),
    );

    println!("Checking region state after procedure completed...");
    let is_dropping = operating_regions.is_dropping(region_id);
    let is_valid = store.is_region_valid(region_id);

    println!("\n=== Results ===");
    println!("Region {} still marked as dropping: {}", region_id, is_dropping);
    println!("Region {} actually valid: {}", region_id, is_valid);

    if !is_dropping && !is_valid {
        println!("\n[FIXED]");
        println!("Consistent state!");
        println!("  - Guard properly released (not dropping)");
        println!("  - Region correctly marked as dropped");
        println!("  - New operations can proceed");
    }

    match reader.read(region_id) {
        Ok(_) => println!("\nUnexpected: read succeeded"),
        Err(e) => println!("\nReader correctly reports: {}", e),
    }
}
