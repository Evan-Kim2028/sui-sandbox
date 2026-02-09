use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;

use super::objects_classifier::analyze_module_object_usage;
use super::objects_profile::{dynamic_confidence_label, resolve_objects_profile};
use super::{
    AnalyzeObjectsCmd, AnalyzeObjectsConfidence, AnalyzeObjectsOutput, ObjectCountSummary,
    ObjectOwnershipCounts, ObjectTypeRow, ObjectTypeStats,
};
use crate::sandbox_cli::SandboxState;
use sui_package_extractor::bytecode::{collect_corpus_package_dirs, read_local_compiled_modules};

impl AnalyzeObjectsCmd {
    pub(super) async fn execute(
        &self,
        _state: &SandboxState,
        verbose: bool,
    ) -> Result<AnalyzeObjectsOutput> {
        let profile = resolve_objects_profile(self)?;
        if verbose {
            eprintln!(
                "[objects] profile={} source={} semantic_mode={} dynamic_mode={} lookback={}",
                profile.name,
                profile.source,
                profile.semantic_mode.as_str(),
                profile.dynamic.mode.as_str(),
                profile.dynamic.lookback
            );
        }
        let package_dirs = collect_corpus_package_dirs(&self.corpus_dir)
            .with_context(|| format!("scan corpus {}", self.corpus_dir.display()))?;
        if package_dirs.is_empty() {
            return Err(anyhow!(
                "no package directories found under {}",
                self.corpus_dir.display()
            ));
        }

        let mut object_stats: BTreeMap<String, ObjectTypeStats> = BTreeMap::new();
        let mut packages_scanned = 0usize;
        let mut packages_failed = 0usize;
        let mut modules_scanned = 0usize;

        for package_dir in package_dirs {
            match read_local_compiled_modules(&package_dir) {
                Ok(modules) => {
                    packages_scanned += 1;
                    modules_scanned += modules.len();
                    for module in &modules {
                        analyze_module_object_usage(module, &mut object_stats, &profile.dynamic);
                    }
                }
                Err(err) => {
                    packages_failed += 1;
                    if verbose {
                        eprintln!("[objects] skip {}: {}", package_dir.display(), err);
                    }
                }
            }
        }

        let mut discovered: Vec<(String, ObjectTypeStats)> = object_stats
            .into_iter()
            .filter(|(_, stats)| stats.key_struct)
            .collect();
        discovered.sort_by(|a, b| a.0.cmp(&b.0));

        let mut ownership = ObjectOwnershipCounts::default();
        let mut ownership_unique = ObjectOwnershipCounts::default();
        let mut party_transfer_eligible = ObjectCountSummary::default();
        let mut party_transfer_observed_in_bytecode = ObjectCountSummary::default();
        let mut singleton_types = 0usize;
        let mut singleton_occurrences = 0usize;
        let mut dynamic_field_types = 0usize;
        let mut dynamic_field_occurrences = 0usize;
        let mut unclassified_types = 0usize;
        let mut multi_mode_types = 0usize;
        let mut immutable_examples = Vec::new();
        let mut party_transfer_eligible_examples = Vec::new();
        let mut party_transfer_eligible_not_observed_examples = Vec::new();
        let mut party_examples = Vec::new();
        let mut receive_examples = Vec::new();
        let mut rows = Vec::new();

        for (type_tag, stats) in &discovered {
            if stats.has_store {
                party_transfer_eligible.types += 1;
                party_transfer_eligible.occurrences += stats.occurrences;
                if party_transfer_eligible_examples.len() < self.top {
                    party_transfer_eligible_examples.push(type_tag.clone());
                }
                if !stats.party && party_transfer_eligible_not_observed_examples.len() < self.top {
                    party_transfer_eligible_not_observed_examples.push(type_tag.clone());
                }
            }

            if stats.owned {
                ownership.owned += stats.occurrences;
                ownership_unique.owned += 1;
            }
            if stats.shared {
                ownership.shared += stats.occurrences;
                ownership_unique.shared += 1;
            }
            if stats.immutable {
                ownership.immutable += stats.occurrences;
                ownership_unique.immutable += 1;
                if immutable_examples.len() < self.top {
                    immutable_examples.push(type_tag.clone());
                }
            }
            if stats.party {
                ownership.party += stats.occurrences;
                ownership_unique.party += 1;
                party_transfer_observed_in_bytecode.types += 1;
                party_transfer_observed_in_bytecode.occurrences += stats.occurrences;
                if party_examples.len() < self.top {
                    party_examples.push(type_tag.clone());
                }
            }
            if stats.receive {
                ownership.receive += stats.occurrences;
                ownership_unique.receive += 1;
                if receive_examples.len() < self.top {
                    receive_examples.push(type_tag.clone());
                }
            }

            let singleton =
                stats.pack_count > 0 && stats.packed_in_init && !stats.packed_outside_init;
            if singleton {
                singleton_types += 1;
                singleton_occurrences += stats.occurrences;
            }
            if stats.dynamic_fields {
                dynamic_field_types += 1;
                dynamic_field_occurrences += stats.occurrences;
            }

            let mode_count = usize::from(stats.owned)
                + usize::from(stats.shared)
                + usize::from(stats.immutable)
                + usize::from(stats.party)
                + usize::from(stats.receive);
            if mode_count == 0 {
                unclassified_types += 1;
            } else if mode_count > 1 {
                multi_mode_types += 1;
            }

            if self.list_types {
                rows.push(ObjectTypeRow {
                    type_tag: type_tag.clone(),
                    party_transfer_eligible: stats.has_store,
                    owned: stats.owned,
                    shared: stats.shared,
                    immutable: stats.immutable,
                    party: stats.party,
                    receive: stats.receive,
                    singleton,
                    dynamic_fields: stats.dynamic_fields,
                });
            }
        }

        Ok(AnalyzeObjectsOutput {
            corpus_dir: self.corpus_dir.display().to_string(),
            profile: profile.clone(),
            packages_scanned,
            packages_failed,
            modules_scanned,
            object_types_discovered: discovered.iter().map(|(_, s)| s.occurrences).sum(),
            object_types_unique: discovered.len(),
            ownership,
            ownership_unique,
            party_transfer_eligible,
            party_transfer_observed_in_bytecode,
            singleton_types,
            singleton_occurrences,
            dynamic_field_types,
            dynamic_field_occurrences,
            unclassified_types,
            multi_mode_types,
            confidence: AnalyzeObjectsConfidence {
                ownership: "medium (static signature + transfer-call heuristics)".to_string(),
                party_metrics:
                    "eligible=high (key+store ability), observed_in_bytecode=medium (party-transfer call sites)"
                        .to_string(),
                singleton: "high (bytecode Pack sites in init vs non-init)".to_string(),
                dynamic_fields: dynamic_confidence_label(&profile),
            },
            immutable_examples,
            party_transfer_eligible_examples,
            party_transfer_eligible_not_observed_examples,
            party_examples,
            receive_examples,
            types: if self.list_types { Some(rows) } else { None },
        })
    }
}

pub(super) fn print_objects_output(output: &AnalyzeObjectsOutput) {
    println!("Object Analysis");
    println!("  Corpus:   {}", output.corpus_dir);
    let profile_path = output
        .profile
        .path
        .as_ref()
        .map(|p| format!(" path={}", p))
        .unwrap_or_default();
    println!(
        "  Profile:  {} (source={}{} semantic_mode={} dynamic_mode={} lookback={})",
        output.profile.name,
        output.profile.source,
        profile_path,
        output.profile.semantic_mode.as_str(),
        output.profile.dynamic.mode.as_str(),
        output.profile.dynamic.lookback
    );
    println!(
        "  Scan:     packages={} failed={} modules={}",
        output.packages_scanned, output.packages_failed, output.modules_scanned
    );
    println!(
        "  Objects:  discovered={} unique={}",
        output.object_types_discovered, output.object_types_unique
    );
    println!(
        "  Ownership (occurrence-weighted) owned={} shared={} immutable={} party={} receive={}",
        output.ownership.owned,
        output.ownership.shared,
        output.ownership.immutable,
        output.ownership.party,
        output.ownership.receive
    );
    println!(
        "  Ownership (unique types)       owned={} shared={} immutable={} party={} receive={}",
        output.ownership_unique.owned,
        output.ownership_unique.shared,
        output.ownership_unique.immutable,
        output.ownership_unique.party,
        output.ownership_unique.receive
    );
    println!(
        "  Party split: eligible(types/occurrences)={}/{} observed_in_bytecode(types/occurrences)={}/{}",
        output.party_transfer_eligible.types,
        output.party_transfer_eligible.occurrences,
        output.party_transfer_observed_in_bytecode.types,
        output.party_transfer_observed_in_bytecode.occurrences
    );
    println!(
        "  Traits:   singleton={} (occurrences={}) dynamic_fields={} (occurrences={}) unclassified={} multi_mode={}",
        output.singleton_types,
        output.singleton_occurrences,
        output.dynamic_field_types,
        output.dynamic_field_occurrences,
        output.unclassified_types,
        output.multi_mode_types
    );
    println!("  Confidence:");
    println!("    ownership: {}", output.confidence.ownership);
    println!("    party:     {}", output.confidence.party_metrics);
    println!("    singleton: {}", output.confidence.singleton);
    println!("    dynamic:   {}", output.confidence.dynamic_fields);
    if !output.party_transfer_eligible_examples.is_empty() {
        println!(
            "  Party-transfer-eligible examples: {}",
            output.party_transfer_eligible_examples.join(", ")
        );
    }
    if !output
        .party_transfer_eligible_not_observed_examples
        .is_empty()
    {
        println!(
            "  Party-transfer-eligible but not observed-in-bytecode examples: {}",
            output
                .party_transfer_eligible_not_observed_examples
                .join(", ")
        );
    }
    if !output.immutable_examples.is_empty() {
        println!(
            "  Immutable examples: {}",
            output.immutable_examples.join(", ")
        );
    }
    if !output.party_examples.is_empty() {
        println!("  Party examples: {}", output.party_examples.join(", "));
    }
    if !output.receive_examples.is_empty() {
        println!("  Receive examples: {}", output.receive_examples.join(", "));
    }
    if let Some(rows) = output.types.as_ref() {
        println!("  Type rows ({}):", rows.len());
        for row in rows.iter().take(20) {
            println!(
                "    {} | eligible={} owned={} shared={} immutable={} party={} receive={} singleton={} dynamic={}",
                row.type_tag,
                row.party_transfer_eligible,
                row.owned,
                row.shared,
                row.immutable,
                row.party,
                row.receive,
                row.singleton,
                row.dynamic_fields
            );
        }
    }
}
