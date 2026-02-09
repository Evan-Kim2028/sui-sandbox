use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Summary of a batch replay run (multiple checkpoints/transactions).
#[derive(Debug, Serialize)]
pub(super) struct BatchReplaySummary {
    pub total_checkpoints: usize,
    pub total_transactions: usize,
    pub total_ptbs: usize,
    pub replayed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_non_ptb: usize,
    /// Per-tag breakdown: tag -> (replayed, succeeded, failed)
    pub by_tag: HashMap<String, (usize, usize, usize)>,
    pub failures: Vec<BatchFailure>,
    /// Per-package breakdown: package_addr -> (replayed, succeeded, failed)
    pub by_package: HashMap<String, (usize, usize, usize)>,
    /// Per-error-category breakdown: category -> count
    pub by_error_category: HashMap<String, usize>,
    /// Successful transaction digests with their packages
    pub successes: Vec<BatchSuccess>,
}

#[derive(Debug, Serialize)]
pub(super) struct BatchFailure {
    pub digest: String,
    pub checkpoint: u64,
    pub error: String,
    pub error_category: String,
    pub tags: Vec<String>,
    pub packages: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct BatchSuccess {
    pub digest: String,
    pub checkpoint: u64,
    pub tags: Vec<String>,
    pub packages: Vec<String>,
}

/// Categorize an error message into a human-readable bucket.
pub(super) fn categorize_error(error: &str) -> String {
    if error.contains("FAILED_TO_DESERIALIZE_ARGUMENT") {
        "FAILED_TO_DESERIALIZE_ARGUMENT".to_string()
    } else if error.contains("LOOKUP_FAILED") {
        "LOOKUP_FAILED".to_string()
    } else if error.contains("UNEXPECTED_VERIFIER_ERROR") {
        "UNEXPECTED_VERIFIER_ERROR".to_string()
    } else if error.contains("ABORTED") {
        // Extract the module info if available
        if let Some(pos) = error.find("sub_status: Some(") {
            let rest = &error[pos + 17..];
            if let Some(end) = rest.find(')') {
                let code = &rest[..end];
                return format!("ABORTED({})", code);
            }
        }
        "ABORTED".to_string()
    } else if error.contains("insufficient balance") {
        "INSUFFICIENT_BALANCE".to_string()
    } else if error.contains("LINKER_ERROR") {
        "LINKER_ERROR".to_string()
    } else if error.contains("Function not found") {
        "FUNCTION_NOT_FOUND".to_string()
    } else if error.contains("object not found") {
        "OBJECT_NOT_FOUND".to_string()
    } else if error.contains("child missing") || error.contains("DF child") {
        "DF_CHILD_MISSING".to_string()
    } else {
        "OTHER".to_string()
    }
}

/// Filter for which digests to replay within a batch.
pub(super) enum DigestFilter {
    /// Replay all PTBs in the checkpoint(s).
    All,
    /// Replay only the specified digests.
    Set(HashSet<String>),
}

impl DigestFilter {
    pub(super) fn parse(digest: &str) -> Self {
        if digest == "*" {
            Self::All
        } else if digest.contains(',') {
            Self::Set(digest.split(',').map(|s| s.trim().to_string()).collect())
        } else {
            Self::Set(std::iter::once(digest.to_string()).collect())
        }
    }

    pub(super) fn matches(&self, digest: &str) -> bool {
        match self {
            Self::All => true,
            Self::Set(set) => set.contains(digest),
        }
    }
}

pub(super) fn print_batch_summary(summary: &BatchReplaySummary) {
    eprintln!();
    eprintln!("\x1b[1m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
    eprintln!("\x1b[1m  Walrus Replay Summary\x1b[0m");
    eprintln!("\x1b[1m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
    eprintln!();

    // --- Overview ---
    let success_rate = if summary.replayed > 0 {
        100.0 * summary.succeeded as f64 / summary.replayed as f64
    } else {
        0.0
    };

    eprintln!("  Checkpoints:    {}", summary.total_checkpoints);
    eprintln!("  Transactions:   {}", summary.total_transactions);
    eprintln!("  PTBs:           {}", summary.total_ptbs);
    eprintln!(
        "  Result:        \x1b[32m{} passed\x1b[0m / \x1b[{}m{} failed\x1b[0m out of {} replayed ({:.1}%)",
        summary.succeeded,
        if summary.failed > 0 { "31" } else { "32" },
        summary.failed,
        summary.replayed,
        success_rate
    );
    eprintln!();

    // --- By Tag ---
    if !summary.by_tag.is_empty() {
        eprintln!("  \x1b[1mBy Transaction Type\x1b[0m");
        let mut tags: Vec<_> = summary.by_tag.iter().collect();
        tags.sort_by(|(_, a), (_, b)| b.0.cmp(&a.0)); // sort by replayed count desc
        for &(tag, &(replayed, succeeded, _failed)) in &tags {
            let rate = if replayed > 0 {
                100.0 * succeeded as f64 / replayed as f64
            } else {
                0.0
            };
            let bar = make_bar(succeeded, replayed, 20);
            eprintln!(
                "    {:<22} {:>3}/{:<3}  {:>5.1}%  {}",
                tag, succeeded, replayed, rate, bar
            );
        }
        eprintln!();
    }

    // --- Failure Breakdown ---
    if !summary.by_error_category.is_empty() {
        eprintln!("  \x1b[1mFailure Breakdown\x1b[0m");
        let mut cats: Vec<_> = summary.by_error_category.iter().collect();
        cats.sort_by(|(_, a), (_, b)| b.cmp(a)); // sort by count desc
        for &(cat, count) in &cats {
            let explanation = match cat.as_str() {
                "FAILED_TO_DESERIALIZE_ARGUMENT" => {
                    "object BCS doesn't match expected type (pre-existing, also fails via gRPC)"
                }
                "LOOKUP_FAILED" | "UNEXPECTED_VERIFIER_ERROR" => {
                    "dependency version conflict across packages (needs per-package link context)"
                }
                "INSUFFICIENT_BALANCE" => {
                    "gas coin balance stale (modified by prior tx in same checkpoint)"
                }
                "LINKER_ERROR" => "module linking failed (missing or incompatible dependency)",
                "FUNCTION_NOT_FOUND" => "called function not found in loaded package",
                "OBJECT_NOT_FOUND" => "required object not in checkpoint data",
                "DF_CHILD_MISSING" => "dynamic field child object not available in checkpoint",
                _ if cat.starts_with("ABORTED(") => {
                    "Move abort (execution-time assertion, often from stale state)"
                }
                "ABORTED" => "Move abort (execution-time assertion, often from stale state)",
                _ => "",
            };
            if explanation.is_empty() {
                eprintln!("    {:>3}x  {}", count, cat);
            } else {
                eprintln!("    {:>3}x  {}", count, cat);
                eprintln!("          \x1b[2m{}\x1b[0m", explanation);
            }
        }
        eprintln!();
    }

    // --- Per-Package Stats ---
    if !summary.by_package.is_empty() {
        eprintln!("  \x1b[1mBy Package (non-system)\x1b[0m");
        let mut pkgs: Vec<_> = summary.by_package.iter().collect();
        pkgs.sort_by(|(_, a), (_, b)| b.0.cmp(&a.0)); // sort by replayed count desc
        let show = 15; // show top N packages
        let total_pkgs = pkgs.len();
        for &(pkg, &(replayed, succeeded, _failed)) in pkgs.iter().take(show) {
            let rate = if replayed > 0 {
                100.0 * succeeded as f64 / replayed as f64
            } else {
                0.0
            };
            // Truncate package address for readability
            let pkg_short = if pkg.len() > 18 {
                format!("{}...{}", &pkg[..10], &pkg[pkg.len() - 4..])
            } else {
                pkg.to_string()
            };
            let bar = make_bar(succeeded, replayed, 12);
            eprintln!(
                "    {:<18} {:>3}/{:<3}  {:>3.0}%  {}",
                pkg_short, succeeded, replayed, rate, bar
            );
        }
        if total_pkgs > show {
            eprintln!(
                "    \x1b[2m... and {} more packages\x1b[0m",
                total_pkgs - show
            );
        }
        eprintln!();
    }

    // --- Successes sample ---
    if !summary.successes.is_empty() {
        let show = 5.min(summary.successes.len());
        eprintln!(
            "  \x1b[1;32mPassing Transactions\x1b[0m (showing {}/{})",
            show,
            summary.successes.len()
        );
        for s in summary.successes.iter().take(show) {
            let pkgs = if s.packages.is_empty() {
                "framework-only".to_string()
            } else {
                s.packages
                    .iter()
                    .map(|p| {
                        if p.len() > 14 {
                            format!("{}...", &p[..14])
                        } else {
                            p.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            eprintln!("    {} (cp {}) [{}]", s.digest, s.checkpoint, pkgs);
        }
        if summary.successes.len() > show {
            eprintln!(
                "    \x1b[2m... {} more\x1b[0m",
                summary.successes.len() - show
            );
        }
        eprintln!();
    }

    // --- Failures detail ---
    if !summary.failures.is_empty() {
        let show = 10.min(summary.failures.len());
        eprintln!(
            "  \x1b[1;31mFailing Transactions\x1b[0m (showing {}/{})",
            show,
            summary.failures.len()
        );
        for f in summary.failures.iter().take(show) {
            let pkgs = if f.packages.is_empty() {
                "framework-only".to_string()
            } else {
                f.packages
                    .iter()
                    .map(|p| {
                        if p.len() > 14 {
                            format!("{}...", &p[..14])
                        } else {
                            p.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            eprintln!(
                "    {} (cp {}) \x1b[31m{}\x1b[0m",
                f.digest, f.checkpoint, f.error_category
            );
            eprintln!("      pkgs=[{}]  tags=[{}]", pkgs, f.tags.join(","));
        }
        if summary.failures.len() > show {
            eprintln!(
                "    \x1b[2m... {} more\x1b[0m",
                summary.failures.len() - show
            );
        }
        eprintln!();
    }

    eprintln!("\x1b[1m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\x1b[0m");
    eprintln!();
}

/// Create a simple progress bar: e.g., "████████░░░░" for 8/12
fn make_bar(filled: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return "░".repeat(width);
    }
    let fill_count = (filled * width + total / 2) / total;
    let fill_count = fill_count.min(width);
    let empty_count = width - fill_count;
    format!(
        "\x1b[32m{}\x1b[0m\x1b[2m{}\x1b[0m",
        "█".repeat(fill_count),
        "░".repeat(empty_count)
    )
}
