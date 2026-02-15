use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::path::Path;

use sui_sandbox_core::health::{run_doctor, DoctorConfig, DoctorReport};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Validate local environment and network endpoints"
)]
pub struct DoctorCmd {
    /// Timeout per network check, in seconds
    #[arg(long, default_value_t = 20)]
    timeout_secs: u64,
}

fn print_report(report: &DoctorReport) {
    println!("sui-sandbox doctor");
    println!("  gRPC:   {}", report.grpc_endpoint);
    println!("  GraphQL: {}", report.graphql_endpoint);
    println!("  Walrus cache: {}", report.walrus_cache_url);
    println!("  Walrus agg:   {}", report.walrus_aggregator_url);
    println!();

    for check in &report.checks {
        let status = if check.passed { "PASS" } else { "FAIL" };
        println!("[{}] {}: {}", status, check.name, check.detail);
        if let Some(remediation) = &check.remediation {
            println!("      fix: {}", remediation);
        }
    }

    println!();
    println!(
        "Summary: {} passed, {} failed",
        report.passed, report.failed
    );
}

impl DoctorCmd {
    pub async fn execute(
        &self,
        state_file: &Path,
        rpc_url: &str,
        json_output: bool,
        _verbose: bool,
    ) -> Result<()> {
        let report = run_doctor(&DoctorConfig {
            timeout_secs: self.timeout_secs,
            rpc_url: rpc_url.to_string(),
            state_file: state_file.to_path_buf(),
            include_toolchain_checks: true,
        })
        .await?;

        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).context("serialize doctor report")?
            );
        } else {
            print_report(&report);
        }

        if report.ok {
            Ok(())
        } else {
            Err(anyhow!("doctor found {} failing checks", report.failed))
        }
    }
}
