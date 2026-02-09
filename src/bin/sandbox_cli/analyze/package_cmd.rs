use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;

use super::mm2_common::{build_mm2_summary, expand_local_modules_for_mm2};
use super::{AnalyzePackageCmd, AnalyzePackageOutput};
use crate::sandbox_cli::network::resolve_graphql_endpoint;
use crate::sandbox_cli::SandboxState;
use sui_package_extractor::bytecode::{
    build_bytecode_interface_value_from_compiled_modules, extract_sanity_counts,
    read_local_compiled_modules,
};
use sui_transport::graphql::GraphQLClient;

impl AnalyzePackageCmd {
    pub(super) async fn execute(
        &self,
        state: &SandboxState,
        verbose: bool,
    ) -> Result<AnalyzePackageOutput> {
        let (package_id, modules, module_names, source) = if let Some(dir) = &self.bytecode_dir {
            let compiled = read_local_compiled_modules(dir)?;
            let pkg_id = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("local")
                .to_string();
            let (module_names, interface_value) =
                build_bytecode_interface_value_from_compiled_modules(&pkg_id, &compiled)?;
            let counts = extract_sanity_counts(
                interface_value
                    .get("modules")
                    .unwrap_or(&serde_json::Value::Null),
            );
            let mm2_modules = if self.mm2 {
                expand_local_modules_for_mm2(dir, state, &compiled, verbose)?
            } else {
                compiled.clone()
            };
            let (mm2_ok, mm2_err) = build_mm2_summary(self.mm2, mm2_modules, verbose);
            return Ok(AnalyzePackageOutput {
                source: "local-bytecode".to_string(),
                package_id: pkg_id,
                modules: counts.modules,
                structs: counts.structs,
                functions: counts.functions,
                key_structs: counts.key_structs,
                module_names: if self.list_modules {
                    Some(module_names)
                } else {
                    None
                },
                mm2_model_ok: mm2_ok,
                mm2_error: mm2_err,
            });
        } else if let Some(pkg_id) = &self.package_id {
            let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            let pkg = graphql
                .fetch_package(pkg_id)
                .with_context(|| format!("fetch package {}", pkg_id))?;
            let mut compiled_modules = Vec::with_capacity(pkg.modules.len());
            let mut names = Vec::with_capacity(pkg.modules.len());
            for module in pkg.modules {
                names.push(module.name.clone());
                let Some(b64) = module.bytecode_base64 else {
                    continue;
                };
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .context("decode module bytecode")?;
                let compiled = CompiledModule::deserialize_with_defaults(&bytes)
                    .context("deserialize module")?;
                compiled_modules.push(compiled);
            }
            names.sort();
            (
                pkg.address,
                compiled_modules,
                if self.list_modules { Some(names) } else { None },
                "graphql".to_string(),
            )
        } else {
            return Err(anyhow!("--package-id or --bytecode-dir is required"));
        };

        let (mm2_ok, mm2_err) = build_mm2_summary(self.mm2, modules.clone(), verbose);
        let counts = {
            let (_, interface_value) =
                build_bytecode_interface_value_from_compiled_modules(&package_id, &modules)?;
            extract_sanity_counts(
                interface_value
                    .get("modules")
                    .unwrap_or(&serde_json::Value::Null),
            )
        };

        Ok(AnalyzePackageOutput {
            source,
            package_id,
            modules: counts.modules,
            structs: counts.structs,
            functions: counts.functions,
            key_structs: counts.key_structs,
            module_names,
            mm2_model_ok: mm2_ok,
            mm2_error: mm2_err,
        })
    }
}

pub(super) fn print_package_output(output: &AnalyzePackageOutput) {
    println!("Package Analysis: {}", output.package_id);
    println!("  Source:   {}", output.source);
    println!(
        "  Counts:   modules={} structs={} functions={} key_structs={}",
        output.modules, output.structs, output.functions, output.key_structs
    );
    if let Some(names) = output.module_names.as_ref() {
        println!("  Modules:  {}", names.join(", "));
    }
    if let Some(ok) = output.mm2_model_ok {
        println!("  MM2:      {}", if ok { "ok" } else { "failed" });
    }
    if let Some(err) = output.mm2_error.as_ref() {
        println!("  MM2 Err:  {}", err);
    }
}
