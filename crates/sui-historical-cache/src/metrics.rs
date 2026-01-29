//! Metrics and reporting for cache operations.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Cache operation metrics (thread-safe counters).
#[derive(Debug, Clone)]
pub struct CacheMetrics {
    /// Objects fetched from Walrus JSON
    pub walrus_hits: Arc<AtomicU64>,
    /// Objects fetched from in-memory cache
    pub memory_hits: Arc<AtomicU64>,
    /// Objects fetched from disk cache
    pub disk_hits: Arc<AtomicU64>,
    /// Objects fetched from gRPC (miss)
    pub grpc_fetches: Arc<AtomicU64>,
    /// Packages loaded from disk cache
    pub package_disk_hits: Arc<AtomicU64>,
    /// Packages fetched from gRPC (miss)
    pub package_grpc_fetches: Arc<AtomicU64>,
    /// Dynamic fields resolved from disk cache
    pub dynamic_field_disk_hits: Arc<AtomicU64>,
    /// Dynamic fields resolved from gRPC (miss)
    pub dynamic_field_grpc_fetches: Arc<AtomicU64>,
}

impl Default for CacheMetrics {
    fn default() -> Self {
        Self {
            walrus_hits: Arc::new(AtomicU64::new(0)),
            memory_hits: Arc::new(AtomicU64::new(0)),
            disk_hits: Arc::new(AtomicU64::new(0)),
            grpc_fetches: Arc::new(AtomicU64::new(0)),
            package_disk_hits: Arc::new(AtomicU64::new(0)),
            package_grpc_fetches: Arc::new(AtomicU64::new(0)),
            dynamic_field_disk_hits: Arc::new(AtomicU64::new(0)),
            dynamic_field_grpc_fetches: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl CacheMetrics {
    /// Record a Walrus hit.
    pub fn record_walrus_hit(&self) {
        self.walrus_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a memory cache hit.
    pub fn record_memory_hit(&self) {
        self.memory_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a disk cache hit.
    pub fn record_disk_hit(&self) {
        self.disk_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a gRPC fetch (miss).
    pub fn record_grpc_fetch(&self) {
        self.grpc_fetches.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a package disk hit.
    pub fn record_package_disk_hit(&self) {
        self.package_disk_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a package gRPC fetch.
    pub fn record_package_grpc_fetch(&self) {
        self.package_grpc_fetches.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a dynamic field disk hit.
    pub fn record_dynamic_field_disk_hit(&self) {
        self.dynamic_field_disk_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a dynamic field gRPC fetch.
    pub fn record_dynamic_field_grpc_fetch(&self) {
        self.dynamic_field_grpc_fetches
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get a snapshot of current metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            walrus_hits: self.walrus_hits.load(Ordering::Relaxed),
            memory_hits: self.memory_hits.load(Ordering::Relaxed),
            disk_hits: self.disk_hits.load(Ordering::Relaxed),
            grpc_fetches: self.grpc_fetches.load(Ordering::Relaxed),
            package_disk_hits: self.package_disk_hits.load(Ordering::Relaxed),
            package_grpc_fetches: self.package_grpc_fetches.load(Ordering::Relaxed),
            dynamic_field_disk_hits: self.dynamic_field_disk_hits.load(Ordering::Relaxed),
            dynamic_field_grpc_fetches: self.dynamic_field_grpc_fetches.load(Ordering::Relaxed),
        }
    }

    /// Reset all counters.
    pub fn reset(&self) {
        self.walrus_hits.store(0, Ordering::Relaxed);
        self.memory_hits.store(0, Ordering::Relaxed);
        self.disk_hits.store(0, Ordering::Relaxed);
        self.grpc_fetches.store(0, Ordering::Relaxed);
        self.package_disk_hits.store(0, Ordering::Relaxed);
        self.package_grpc_fetches.store(0, Ordering::Relaxed);
        self.dynamic_field_disk_hits.store(0, Ordering::Relaxed);
        self.dynamic_field_grpc_fetches.store(0, Ordering::Relaxed);
    }
}

/// Snapshot of metrics (for reporting).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub walrus_hits: u64,
    pub memory_hits: u64,
    pub disk_hits: u64,
    pub grpc_fetches: u64,
    pub package_disk_hits: u64,
    pub package_grpc_fetches: u64,
    pub dynamic_field_disk_hits: u64,
    pub dynamic_field_grpc_fetches: u64,
}

impl MetricsSnapshot {
    /// Total object lookups.
    pub fn total_object_lookups(&self) -> u64 {
        self.walrus_hits + self.memory_hits + self.disk_hits + self.grpc_fetches
    }

    /// Total package lookups.
    pub fn total_package_lookups(&self) -> u64 {
        self.package_disk_hits + self.package_grpc_fetches
    }

    /// Cache hit rate for objects (excluding Walrus, which is always first).
    pub fn object_cache_hit_rate(&self) -> f64 {
        let total = self.memory_hits + self.disk_hits + self.grpc_fetches;
        if total == 0 {
            return 0.0;
        }
        (self.memory_hits + self.disk_hits) as f64 / total as f64
    }

    /// Disk cache hit rate for objects.
    pub fn object_disk_hit_rate(&self) -> f64 {
        let total = self.memory_hits + self.disk_hits + self.grpc_fetches;
        if total == 0 {
            return 0.0;
        }
        self.disk_hits as f64 / total as f64
    }

    /// Package cache hit rate.
    pub fn package_hit_rate(&self) -> f64 {
        let total = self.total_package_lookups();
        if total == 0 {
            return 0.0;
        }
        self.package_disk_hits as f64 / total as f64
    }

    /// Format a human-readable report.
    pub fn format_report(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Cache Metrics Report".to_string());
        lines.push("=".repeat(50));
        lines.push("Object Lookups:".to_string());
        lines.push(format!("  Walrus JSON:     {}", self.walrus_hits));
        lines.push(format!("  Memory Cache:    {}", self.memory_hits));
        lines.push(format!("  Disk Cache:      {}", self.disk_hits));
        lines.push(format!("  gRPC (miss):     {}", self.grpc_fetches));
        lines.push(format!(
            "  Cache Hit Rate:  {:.1}%",
            self.object_cache_hit_rate() * 100.0
        ));
        lines.push(format!(
            "  Disk Hit Rate:   {:.1}%",
            self.object_disk_hit_rate() * 100.0
        ));
        lines.push(String::new());
        lines.push("Package Lookups:".to_string());
        lines.push(format!("  Disk Cache:      {}", self.package_disk_hits));
        lines.push(format!("  gRPC (miss):     {}", self.package_grpc_fetches));
        lines.push(format!(
            "  Hit Rate:        {:.1}%",
            self.package_hit_rate() * 100.0
        ));
        lines.push(String::new());
        lines.push("Dynamic Fields:".to_string());
        lines.push(format!(
            "  Disk Cache:      {}",
            self.dynamic_field_disk_hits
        ));
        lines.push(format!(
            "  gRPC (miss):     {}",
            self.dynamic_field_grpc_fetches
        ));
        lines.join("\n")
    }
}
