//! Generate Move source stubs from bytecode interface.
//!
//! These stubs allow the Move compiler to type-check code that imports types and calls
//! functions from a package, even when only bytecode is available.
//!
//! Key features:
//! - Proper `use` imports for external types (std::, sui::, etc.)
//! - Correct Move 2024 syntax for all type positions
//! - Struct fields use unqualified names after imports
//! - Function bodies are `abort 0` stubs (never executed)

use anyhow::{Context, Result};
use move_binary_format::file_format::{
    Ability, AbilitySet, CompiledModule, DatatypeHandleIndex, SignatureToken,
    StructFieldInformation, Visibility,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Well-known framework addresses (without 0x prefix, lowercase)
const MOVE_STDLIB_ADDR: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SUI_FRAMEWORK_ADDR: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const SUI_SYSTEM_ADDR: &str = "0000000000000000000000000000000000000000000000000000000000000003";

/// Represents an external type that needs to be imported
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ExternalType {
    /// Package prefix: "std", "sui", "sui_system", or a custom address
    package: String,
    /// Module name within the package
    module: String,
    /// Type name
    name: String,
}

impl ExternalType {
    fn import_line(&self) -> String {
        format!("    use {}::{}::{};", self.package, self.module, self.name)
    }
}

/// Context for generating Move stubs for a single module
struct StubContext<'a> {
    module: &'a CompiledModule,
    /// External types that need imports (collected during type traversal)
    imports: BTreeSet<ExternalType>,
    /// Package alias for self-referential types
    pkg_alias: String,
    /// Self module name
    self_module: String,
    /// Self address hex (for detecting self-references)
    self_addr_hex: String,
}

impl<'a> StubContext<'a> {
    fn new(module: &'a CompiledModule, pkg_alias: &str) -> Self {
        let module_id = module.self_id();
        let self_module = module_id.name().as_str().to_string();
        let self_addr_hex = format!("{:0>64}", hex_encode_lower(module_id.address().as_ref()));

        Self {
            module,
            imports: BTreeSet::new(),
            pkg_alias: pkg_alias.to_string(),
            self_module,
            self_addr_hex,
        }
    }

    /// Convert a signature token to Move source syntax.
    /// If `use_qualified` is true (for function params/returns), uses fully-qualified paths.
    /// If false (for struct fields), uses unqualified names that rely on imports.
    fn token_to_move(&mut self, token: &SignatureToken, use_qualified: bool) -> String {
        match token {
            SignatureToken::Bool => "bool".to_string(),
            SignatureToken::U8 => "u8".to_string(),
            SignatureToken::U16 => "u16".to_string(),
            SignatureToken::U32 => "u32".to_string(),
            SignatureToken::U64 => "u64".to_string(),
            SignatureToken::U128 => "u128".to_string(),
            SignatureToken::U256 => "u256".to_string(),
            SignatureToken::Address => "address".to_string(),
            SignatureToken::Signer => "signer".to_string(),
            SignatureToken::Vector(inner) => {
                let inner_str = self.token_to_move(inner, use_qualified);
                format!("vector<{}>", inner_str)
            }
            SignatureToken::TypeParameter(idx) => format!("T{}", idx),
            SignatureToken::Reference(inner) => {
                let inner_str = self.token_to_move(inner, use_qualified);
                format!("&{}", inner_str)
            }
            SignatureToken::MutableReference(inner) => {
                let inner_str = self.token_to_move(inner, use_qualified);
                format!("&mut {}", inner_str)
            }
            SignatureToken::Datatype(idx) => self.datatype_to_move(*idx, &[], use_qualified),
            SignatureToken::DatatypeInstantiation(inst) => {
                let (idx, type_args) = &**inst;
                self.datatype_to_move(*idx, type_args, use_qualified)
            }
        }
    }

    fn datatype_to_move(
        &mut self,
        idx: DatatypeHandleIndex,
        type_args: &[SignatureToken],
        use_qualified: bool,
    ) -> String {
        let handle = self.module.datatype_handle_at(idx);
        let module_handle = self.module.module_handle_at(handle.module);
        let addr = self.module.address_identifier_at(module_handle.address);
        let module_name = self.module.identifier_at(module_handle.name).as_str();
        let struct_name = self.module.identifier_at(handle.name).as_str();

        let addr_hex = format!("{:0>64}", hex_encode_lower(addr.as_ref()));

        // Determine the package prefix
        let (package, is_self) = if addr_hex == MOVE_STDLIB_ADDR {
            ("std".to_string(), false)
        } else if addr_hex == SUI_FRAMEWORK_ADDR {
            ("sui".to_string(), false)
        } else if addr_hex == SUI_SYSTEM_ADDR {
            ("sui_system".to_string(), false)
        } else if addr_hex == self.self_addr_hex {
            // Same package (self-reference)
            (self.pkg_alias.clone(), true)
        } else {
            // External package - use hex address
            (format!("0x{}", addr_hex), false)
        };

        // Build type args string
        let type_args_str = if type_args.is_empty() {
            String::new()
        } else {
            let args: Vec<String> = type_args
                .iter()
                .map(|t| self.token_to_move(t, use_qualified))
                .collect();
            format!("<{}>", args.join(", "))
        };

        // Decide whether to use qualified or unqualified name
        if is_self && module_name == self.self_module {
            // Same module - just use the type name (no import needed)
            format!("{}{}", struct_name, type_args_str)
        } else if is_self {
            // Same package, different module
            // Add import for cross-module references within the same package
            self.imports.insert(ExternalType {
                package: package.clone(),
                module: module_name.to_string(),
                name: struct_name.to_string(),
            });
            if use_qualified {
                // Function position - use full path
                format!(
                    "{}::{}::{}{}",
                    package, module_name, struct_name, type_args_str
                )
            } else {
                // Struct field - use unqualified (relies on import)
                format!("{}{}", struct_name, type_args_str)
            }
        } else {
            // External package type - add import
            self.imports.insert(ExternalType {
                package: package.clone(),
                module: module_name.to_string(),
                name: struct_name.to_string(),
            });
            if use_qualified {
                // External type in function position - use fully qualified
                format!(
                    "{}::{}::{}{}",
                    package, module_name, struct_name, type_args_str
                )
            } else {
                // External type in struct field - use unqualified (relies on import)
                format!("{}{}", struct_name, type_args_str)
            }
        }
    }

    fn ability_set_to_strings(set: &AbilitySet) -> Vec<&'static str> {
        let mut out = Vec::new();
        if set.has_ability(Ability::Copy) {
            out.push("copy");
        }
        if set.has_ability(Ability::Drop) {
            out.push("drop");
        }
        if set.has_ability(Ability::Store) {
            out.push("store");
        }
        if set.has_ability(Ability::Key) {
            out.push("key");
        }
        out
    }
}

/// Simple hex encoding without external crate
fn hex_encode_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Generate Move source stub for a single compiled module.
pub fn generate_module_stub(module: &CompiledModule, pkg_alias: &str) -> Result<String> {
    let mut ctx = StubContext::new(module, pkg_alias);
    let mut lines: Vec<String> = Vec::new();

    let module_id = module.self_id();
    let module_name = module_id.name().as_str();

    // First pass: collect all external types by traversing structs and functions
    // We need to do this before generating code so imports come first

    // Traverse struct fields (these need unqualified names)
    for def in module.struct_defs() {
        if let StructFieldInformation::Declared(fields) = &def.field_information {
            for field in fields {
                // Just collect imports, discard the result
                ctx.token_to_move(&field.signature.0, false);
            }
        }
    }

    // Traverse function params and returns
    for def in module.function_defs() {
        let handle = module.function_handle_at(def.function);
        let params = module.signature_at(handle.parameters);
        let ret = module.signature_at(handle.return_);

        for token in &params.0 {
            ctx.token_to_move(token, true);
        }
        for token in &ret.0 {
            ctx.token_to_move(token, true);
        }
    }

    // Module header
    lines.push(format!("module {}::{} {{", pkg_alias, module_name));
    lines.push(String::new());

    // Generate imports
    if !ctx.imports.is_empty() {
        lines.push("    // External type imports".to_string());
        for ext in &ctx.imports {
            lines.push(ext.import_line());
        }
        lines.push(String::new());
    }

    lines.push("    // Auto-generated stub module for type-checking".to_string());
    lines.push("    // Function bodies abort - real bytecode used at runtime".to_string());
    lines.push(String::new());

    // Generate struct declarations
    for def in module.struct_defs() {
        let handle = module.datatype_handle_at(def.struct_handle);
        let name = module.identifier_at(handle.name).as_str();
        let abilities = StubContext::ability_set_to_strings(&handle.abilities);

        // Type parameters
        let type_params: Vec<String> = handle
            .type_parameters
            .iter()
            .enumerate()
            .map(|(i, tp)| {
                let mut s = String::new();
                if tp.is_phantom {
                    s.push_str("phantom ");
                }
                s.push_str(&format!("T{}", i));
                let constraints = StubContext::ability_set_to_strings(&tp.constraints);
                if !constraints.is_empty() {
                    s.push_str(": ");
                    s.push_str(&constraints.join(" + "));
                }
                s
            })
            .collect();

        let tp_decl = if type_params.is_empty() {
            String::new()
        } else {
            format!("<{}>", type_params.join(", "))
        };

        let abilities_str = if abilities.is_empty() {
            String::new()
        } else {
            format!(" has {}", abilities.join(", "))
        };

        match &def.field_information {
            StructFieldInformation::Native => {
                lines.push(format!(
                    "    public struct {}{}{}",
                    name, tp_decl, abilities_str
                ));
                lines.push("        has native;".to_string());
            }
            StructFieldInformation::Declared(fields) => {
                lines.push(format!(
                    "    public struct {}{}{} {{",
                    name, tp_decl, abilities_str
                ));
                for field in fields {
                    let field_name = module.identifier_at(field.name).as_str();
                    // Use unqualified names for struct fields (relies on imports)
                    let field_type = ctx.token_to_move(&field.signature.0, false);
                    lines.push(format!("        {}: {},", field_name, field_type));
                }
                lines.push("    }".to_string());
            }
        }
        lines.push(String::new());
    }

    // Generate function declarations
    for def in module.function_defs() {
        let handle = module.function_handle_at(def.function);
        let name = module.identifier_at(handle.name).as_str();

        // Skip private functions - they can't be called externally
        if def.visibility == Visibility::Private {
            continue;
        }

        let vis_str = match def.visibility {
            Visibility::Public => "public",
            Visibility::Friend => "public(package)",
            Visibility::Private => continue, // Already handled above
        };

        let entry_str = if def.is_entry { " entry" } else { "" };

        // Type parameters
        let type_params: Vec<String> = handle
            .type_parameters
            .iter()
            .enumerate()
            .map(|(i, tp)| {
                let mut s = format!("T{}", i);
                let constraints = StubContext::ability_set_to_strings(tp);
                if !constraints.is_empty() {
                    s.push_str(": ");
                    s.push_str(&constraints.join(" + "));
                }
                s
            })
            .collect();

        let tp_decl = if type_params.is_empty() {
            String::new()
        } else {
            format!("<{}>", type_params.join(", "))
        };

        // Parameters
        let params = module.signature_at(handle.parameters);
        let param_strs: Vec<String> = params
            .0
            .iter()
            .enumerate()
            .map(|(i, token)| {
                let ty = ctx.token_to_move(token, true);
                format!("_p{}: {}", i, ty)
            })
            .collect();
        let params_decl = param_strs.join(", ");

        // Return type
        let ret = module.signature_at(handle.return_);
        let ret_decl = if ret.0.is_empty() {
            String::new()
        } else if ret.0.len() == 1 {
            format!(": {}", ctx.token_to_move(&ret.0[0], true))
        } else {
            let ret_types: Vec<String> = ret.0.iter().map(|t| ctx.token_to_move(t, true)).collect();
            format!(": ({})", ret_types.join(", "))
        };

        // Check if native
        let is_native = def.code.is_none();

        if is_native {
            lines.push(format!(
                "    {}{} native fun {}{}({}){};",
                vis_str, entry_str, name, tp_decl, params_decl, ret_decl
            ));
        } else {
            lines.push(format!(
                "    {}{} fun {}{}({}){} {{",
                vis_str, entry_str, name, tp_decl, params_decl, ret_decl
            ));
            lines.push("        abort 0".to_string());
            lines.push("    }".to_string());
        }
        lines.push(String::new());
    }

    lines.push("}".to_string());

    Ok(lines.join("\n"))
}

/// Generate Move source stubs for all modules and write to a directory.
pub fn emit_move_stubs(
    modules: &[CompiledModule],
    pkg_alias: &str,
    out_dir: &Path,
) -> Result<BTreeMap<String, String>> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create stubs directory: {}", out_dir.display()))?;

    let mut stubs = BTreeMap::new();

    for module in modules {
        let module_name = module.self_id().name().as_str().to_string();
        let stub = generate_module_stub(module, pkg_alias)
            .with_context(|| format!("failed to generate stub for module {}", module_name))?;

        let file_path = out_dir.join(format!("{}.move", module_name));
        fs::write(&file_path, &stub)
            .with_context(|| format!("failed to write stub file: {}", file_path.display()))?;

        stubs.insert(module_name, stub);
    }

    Ok(stubs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_external_type_import_line() {
        let ext = ExternalType {
            package: "std".to_string(),
            module: "type_name".to_string(),
            name: "TypeName".to_string(),
        };
        assert_eq!(ext.import_line(), "    use std::type_name::TypeName;");
    }

    #[test]
    fn test_hex_encode_lower() {
        assert_eq!(hex_encode_lower(&[0x00, 0x01, 0x02]), "000102");
        assert_eq!(hex_encode_lower(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
