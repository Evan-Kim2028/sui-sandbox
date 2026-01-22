#!/bin/bash
#
# Update Sui version across the project
#
# Usage: ./scripts/update-sui-version.sh mainnet-v1.XX.X
#
# This script:
# 1. Updates all version references in Cargo.toml
# 2. Updates the version constant in src/grpc/version.rs
# 3. Fetches new proto definitions from the Sui repository
# 4. Provides instructions for remaining manual steps

set -e

NEW_VERSION="${1:-}"

if [ -z "$NEW_VERSION" ]; then
    echo "Usage: $0 <new-version>"
    echo "Example: $0 mainnet-v1.70.0"
    exit 1
fi

# Validate version format
if [[ ! "$NEW_VERSION" =~ ^mainnet-v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must match format 'mainnet-vX.XX.X'"
    echo "Example: mainnet-v1.70.0"
    exit 1
fi

echo "=== Sui Version Update Script ==="
echo "Updating to: $NEW_VERSION"
echo ""

# Get current version from Cargo.toml
CURRENT_VERSION=$(grep -oP 'tag = "\K[^"]+' Cargo.toml | head -1)
echo "Current version: $CURRENT_VERSION"
echo ""

if [ "$CURRENT_VERSION" = "$NEW_VERSION" ]; then
    echo "Already at version $NEW_VERSION, nothing to do."
    exit 0
fi

# Step 1: Update Cargo.toml
echo "Step 1: Updating Cargo.toml..."
sed -i "s/tag = \"$CURRENT_VERSION\"/tag = \"$NEW_VERSION\"/g" Cargo.toml
echo "  Updated $(grep -c "tag = \"$NEW_VERSION\"" Cargo.toml) references"

# Step 2: Update version.rs
echo "Step 2: Updating src/grpc/version.rs..."
sed -i "s/PINNED_SUI_VERSION: &str = \"$CURRENT_VERSION\"/PINNED_SUI_VERSION: \&str = \"$NEW_VERSION\"/" src/grpc/version.rs
echo "  Updated PINNED_SUI_VERSION constant"

# Step 3: Fetch new proto definitions
echo "Step 3: Fetching proto definitions..."
TEMP_DIR=$(mktemp -d)
echo "  Cloning Sui repository (sparse checkout)..."

cd "$TEMP_DIR"
git init -q
git remote add origin https://github.com/MystenLabs/sui.git
git config core.sparseCheckout true
echo "crates/sui-rpc-api/proto/" > .git/info/sparse-checkout
git fetch --depth 1 origin "$NEW_VERSION" 2>/dev/null || {
    echo "  Warning: Could not fetch tag $NEW_VERSION"
    echo "  Proto update skipped - you may need to update manually"
    cd -
    rm -rf "$TEMP_DIR"
    PROTO_UPDATED=false
}

if [ -d "crates/sui-rpc-api/proto" ]; then
    cd -
    echo "  Copying proto files..."
    cp -r "$TEMP_DIR/crates/sui-rpc-api/proto/sui" proto/
    rm -rf "$TEMP_DIR"
    PROTO_UPDATED=true
    echo "  Proto files updated"
else
    cd - 2>/dev/null || true
    PROTO_UPDATED=false
fi

echo ""
echo "=== Update Summary ==="
echo "  Cargo.toml: Updated"
echo "  version.rs: Updated"
echo "  Proto files: $([ "$PROTO_UPDATED" = true ] && echo "Updated" || echo "Skipped (update manually)")"
echo ""
echo "=== Remaining Manual Steps ==="
echo ""
echo "1. Update Dockerfile SUI_VERSION:"
echo "   ARG SUI_VERSION=$NEW_VERSION"
echo ""
echo "2. Regenerate proto Rust code:"
echo "   cargo build"
echo ""
echo "3. Rebuild framework bytecode:"
echo "   docker build -t sui-extractor ."
echo "   docker run --rm sui-extractor cat /framework_bytecode/move-stdlib > framework_bytecode/move-stdlib"
echo "   docker run --rm sui-extractor cat /framework_bytecode/sui-framework > framework_bytecode/sui-framework"
echo "   docker run --rm sui-extractor cat /framework_bytecode/sui-system > framework_bytecode/sui-system"
echo ""
echo "4. Run tests:"
echo "   cargo test"
echo ""
echo "5. Update compatibility matrix in src/grpc/version.rs if needed"
echo ""
echo "Done! Review changes with: git diff"
