#!/usr/bin/env python3
"""Genera packaging/THIRD-PARTY-LICENSES.md: las dependencias de terceros del workspace, con su
licencia declarada en Cargo. Uso (desde la raíz del repo): python3 packaging/licenses.py

Es un inventario, no los textos completos: los textos viven en el código fuente de cada crate
(`~/.cargo/registry`). Cuando se instale cargo-about, puede reemplazar a este script.
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "packaging" / "THIRD-PARTY-LICENSES.md"

# Piezas que no son crates de Cargo (se embeben en el binario o en el AppImage).
EMBEDDED = [
    ("yt-dlp/ejs (scripts solucionador)", "0.8.0", "Unlicense (meriyah: ISC; astring: MIT)",
     "https://github.com/yt-dlp/ejs"),
    ("Inter (fuente, recortada a latín)", "ver app/assets/fonts", "SIL Open Font License 1.1", "app/assets/fonts/OFL.txt"),
    ("mpv", "del sistema (no va en el AppImage)", "GPL-2.0-or-later", "https://mpv.io"),
]


def main() -> int:
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=ROOT))
    members = set(meta["workspace_members"])
    packages = sorted(
        (p for p in meta["packages"] if p["id"] not in members),
        key=lambda p: (p["name"].lower(), p["version"]),
    )
    lines = [
        "# Licencias de terceros",
        "",
        "Generado por `packaging/licenses.py` desde `cargo metadata --locked`. Cada crate conserva su",
        "licencia; los textos completos están en el código fuente de cada uno.",
        "",
        "## Piezas embebidas",
        "",
        "| Pieza | Versión | Licencia | Origen |",
        "|---|---|---|---|",
    ]
    for name, version, lic, origin in EMBEDDED:
        lines.append(f"| {name} | {version} | {lic} | {origin} |")
    lines += [
        "",
        "## Crates de Cargo",
        "",
        f"{len(packages)} crates. Slint se usa bajo GPL-3.0 (una de sus tres licencias).",
        "",
        "| Crate | Versión | Licencia | Repositorio |",
        "|---|---|---|---|",
    ]
    for p in packages:
        lic = (p.get("license") or "sin licencia declarada").replace("|", "/")
        repo = p.get("repository") or ""
        lines.append(f"| {p['name']} | {p['version']} | {lic} | {repo} |")
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{OUT.relative_to(ROOT)}: {len(packages)} crates", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
