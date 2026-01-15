# Error Codes Reference

Complete reference for all error codes in the sandbox.

## Error Code Format

Errors follow the pattern `EXXX` where the first digit indicates the phase:

| Phase | Code Range | Description |
|-------|------------|-------------|
| Build | E001-E006 | Move compiler errors |
| Resolution | E101-E103 | Module/function lookup |
| TypeCheck | E201-E205 | Static type validation |
| Synthesis | E301-E304 | Argument construction |
| Execution | E401-E404 | VM execution |
| Validation | E501-E502 | Result verification |

---

## Build Errors (E001-E006)

Errors during Move code compilation.

### E001: ModuleAddressUndefined

Module address not defined in Move.toml.

### E002: InvalidManifest

Invalid Move.toml syntax or missing required fields.

### E003: ImportResolutionFailed

A `use` statement references a non-existent module or type.

### E004: TypeSyntaxError

Invalid type syntax in code (e.g., malformed generic parameters).

### E005: EntryFunctionInvalid

An `entry` function has an invalid signature.

### E006: CompileTimeAbilityError

A type is used in a context that requires abilities it doesn't have.

---

## Resolution Errors (E101-E103)

Errors during module/function lookup.

### E101: ModuleNotFound

The requested module doesn't exist in loaded packages.

### E102: FunctionNotFound

The function name doesn't exist in the specified module.

### E103: FunctionNotCallable

Function exists but is not callable (private or lacks required visibility).

---

## TypeCheck Errors (E201-E205)

Errors during static type validation.

### E201: TypeMismatch

Argument type doesn't match function parameter type.

### E202: AbilityViolation

Using a type in a context that requires abilities it doesn't have.

### E203: GenericBoundsNotSatisfied

Type argument doesn't satisfy generic constraints.

### E204: RecursiveType

A type references itself, making layout computation impossible.

### E205: UnknownType

Referenced struct doesn't exist in any loaded module.

---

## Synthesis Errors (E301-E304)

Errors during argument value construction.

### E301: NoConstructor

Cannot find a function that returns the required type.

### E302: ChainDepthExceeded

Building the type requires too many nested constructor calls.

### E303: UnsupportedParameter

A required constructor needs a type that can't be synthesized.

### E304: SerializationFailed

Value couldn't be serialized to BCS format.

---

## Execution Errors (E401-E404)

Errors during VM execution.

### E401: VMSetupFailed

Internal error creating the VM execution environment.

### E402: ConstructorAborted

A constructor function called `abort` during execution.

**Details include:**
- `abort_code`: The numeric abort code
- `module`: Which module aborted
- `function`: Which function aborted

### E403: TargetAborted

The main function called `abort` during execution.

**Details include:**
- `abort_code`: The numeric abort code
- `module`: Which module aborted
- `function`: Which function aborted

### E404: UnsupportedNative

Code called a native function that's mocked or unavailable in sandbox.

---

## Validation Errors (E501-E502)

Errors during result verification.

### E501: NoTargetAccess

Execution completed but never called into the target package.

### E502: ReturnTypeMismatch

Function returned a value of unexpected type.

---

## Sandbox-Specific Errors

These errors come from `SimulationError` and are specific to sandbox execution:

### MissingPackage

PTB references a package that isn't loaded.

**Fields:**
- `address`: The missing package address
- `module`: Optional module name within the package

### MissingObject

PTB references an object that doesn't exist in sandbox state.

**Fields:**
- `id`: The missing object ID
- `expected_type`: Optional expected type of the object

### TypeMismatch

Type mismatch between expected and provided values.

**Fields:**
- `expected`: The expected type
- `got`: The actual type
- `location`: Where the mismatch occurred

### ContractAbort

Move code called `abort` with a code.

**Fields:**
- `abort_code`: Numeric abort code
- `module`: Where abort occurred
- `function`: Which function aborted
- `message`: Optional message

### DeserializationFailed

Argument could not be deserialized.

**Fields:**
- `argument_index`: Which argument failed
- `expected_type`: What type was expected

### SharedObjectLockConflict

Shared object is locked by another transaction.

**Fields:**
- `object_id`: The locked object
- `held_by`: Optional transaction holding the lock
- `reason`: Description of the conflict
