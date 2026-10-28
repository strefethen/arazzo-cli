#!/usr/bin/env python3
"""Apply the dated experiment to a disposable snapshot; never edit source."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true", help="run the focused offline tests")
    args = parser.parse_args()
    artifact = Path(__file__).resolve().parent
    source = artifact.parents[2]
    baseline = json.loads((artifact / "baseline.json").read_text())
    for relative, expected in baseline["sha256"].items():
        path = source / relative
        if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise SystemExit(f"Baseline changed: {relative}; review the experiment before reapplying.")
    for relative in baseline["new_files"]:
        if (source / relative).exists():
            raise SystemExit(f"Experiment file already exists in source: {relative}")
    destination = Path(tempfile.mkdtemp(prefix="arazzo-message-spike-"))
    for relative in (
        "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "AGENTS.md", "crates",
        "vendor", "spec", "testdata", "examples", "tests", "docs/schemas",
    ):
        original = source / relative
        target = destination / relative
        if not original.exists():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if original.is_dir():
            shutil.copytree(original, target, ignore=shutil.ignore_patterns("target", "node_modules"))
        else:
            shutil.copy2(original, target)
    patch = str(artifact / "experiment.patch")
    subprocess.run(["git", "apply", "--check", patch], cwd=destination, check=True)
    subprocess.run(["git", "apply", patch], cwd=destination, check=True)
    print(f"Disposable snapshot: {destination}", flush=True)
    command = ["cargo", "test", "--offline", "-p", "arazzo-runtime", "--lib", "message_spike"]
    if args.test:
        subprocess.run(command, cwd=destination, check=True)
    else:
        print("Run in that directory: " + " ".join(command))


if __name__ == "__main__":
    main()
