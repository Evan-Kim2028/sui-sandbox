#!/usr/bin/env python3
import os, sui_sandbox

VERSIONS_FILE = os.getenv(
    "VERSIONS_FILE",
    "examples/data/deepbook_margin_state/deepbook_versions_240733000.json",
)
MARGIN_PKG = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b"
SPOT_PKG = "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497"
SUI = "0x2::sui::SUI"
USDC = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC"
OBJS = ["0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80", "0x0e40998b359a9ccbab22a98ed21bd4346abf19158bc7980c8291908086b3a742", "0x801dbc2f0053d34734814b2d6df491ce7807a725fe9a01ad74a07e9c51396c37", "0x5dec622733a204ca27f5a90d8c2fad453cc6665186fd5dff13a83d0b6c9027ab", "0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407", "0x53041c6f86c4782aabbfc1d4fe234a6d37160310c7ee740c915f0a01b7127344", "0xba473d9ae278f10af75c50a8fa341e9c6a1c087dc91a3f23e8048baf67d0754f", "0x6"]

out = sui_sandbox.historical_view_from_versions(versions_file=VERSIONS_FILE, package_id=MARGIN_PKG, module="margin_manager", function="manager_state", required_objects=OBJS, type_args=[SUI, USDC], package_roots=[MARGIN_PKG, SPOT_PKG], type_refs=[SUI, USDC], grpc_endpoint=os.getenv("SUI_GRPC_ENDPOINT"), grpc_api_key=os.getenv("SUI_GRPC_API_KEY"))
SCHEMA = [{"index": 2, "name": "risk_ratio_pct", "type_hint": "u64", "scale": 1e7}, {"index": 3, "name": "base_asset_sui", "type_hint": "u64", "scale": 1e9}, {"index": 4, "name": "quote_asset_usdc", "type_hint": "u64", "scale": 1e6}, {"index": 5, "name": "base_debt_sui", "type_hint": "u64", "scale": 1e9}, {"index": 6, "name": "quote_debt_usdc", "type_hint": "u64", "scale": 1e6}, {"index": 11, "name": "current_price", "type_hint": "u64", "scale": 1e6}]
decoded = sui_sandbox.historical_decode_with_schema(out, SCHEMA) if out.get("success") else None
print({"success": out.get("success"), "gas_used": out.get("gas_used"), "decoded": decoded, "hint": out.get("hint")})
