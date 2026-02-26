"""Build script for vs20XX_win-{64,arm64} packages.

Generates msvcup.toml config, copies activation scripts,
and runs msvcup resolve to create the lock file and shims.
"""

import os
import subprocess
import sys
from pathlib import Path

prefix = Path(os.environ["PREFIX"])
recipe_dir = Path(os.environ["RECIPE_DIR"])

# Usage: install_vs.py <target_arch> <msvc_version> <sdk_version> <vs_year> <vs_ver>
target_arch, msvc_version, sdk_version, vs_year, vs_ver = sys.argv[1:6]

# Create directories
(prefix / "etc" / "conda" / "activate.d").mkdir(parents=True, exist_ok=True)
(prefix / "etc" / "conda" / "deactivate.d").mkdir(parents=True, exist_ok=True)
(prefix / "bin").mkdir(parents=True, exist_ok=True)

# Write msvcup.toml — omit install_dir and cache_dir so they resolve
# at runtime from USERPROFILE (defaults in the autoenv binary)
toml_path = prefix / "bin" / "msvcup.toml"
toml_path.write_text(f"""\
[msvcup]
lock_file = "msvc.lock"
target_arch = "{target_arch}"

[packages]
msvc = "{msvc_version}"
sdk = "{sdk_version}"
""")

# Run msvcup resolve
subprocess.check_call([
    "msvcup", "resolve",
    "--config", str(toml_path),
    "--out-dir", str(prefix / "bin"),
    "--manifest-update", "always",
])

# Copy activation / deactivation scripts
activate_src = recipe_dir / "activate.bat"
deactivate_src = recipe_dir / "deactivate.bat"

activate_dst = prefix / "etc" / "conda" / "activate.d" / f"vs{vs_year}_activate.bat"
deactivate_dst = prefix / "etc" / "conda" / "deactivate.d" / f"vs{vs_year}_deactivate.bat"

# Substitute VS_VER and VS_YEAR placeholders
for src, dst in [(activate_src, activate_dst), (deactivate_src, deactivate_dst)]:
    text = src.read_text()
    text = text.replace("%VS_VER%", vs_ver).replace("%VS_YEAR%", vs_year)
    dst.write_text(text)
