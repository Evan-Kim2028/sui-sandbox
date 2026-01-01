from __future__ import annotations

from pathlib import Path


def load_dotenv(path: Path) -> dict[str, str]:
    """
    Minimal .env loader:
    - supports KEY=VALUE
    - strips surrounding quotes
    - ignores blank lines and `#` comments
    - does not expand variables
    """
    out: dict[str, str] = {}
    if not path.exists():
        return out

    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        k, v = line.split("=", 1)
        k = k.strip()
        v = v.strip()
        if not k:
            continue
        if len(v) >= 2 and ((v[0] == v[-1] == '"') or (v[0] == v[-1] == "'")):
            v = v[1:-1]
        out[k] = v
    return out

