#!/usr/bin/env python3
"""Copy the PyO3 cdylib into the aidb package.

Used by pip/CI. Users run `pip install aidb` — they do not copy a dylib.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PKG = HERE.parent
REPO = PKG.parent.parent
AIDB_PKG = PKG / "aidb"


def cargo_names() -> list[str]:
    if sys.platform == "darwin":
        return ["libaidb_native.dylib", "aidb_native.so", "aidb_native.abi3.so"]
    if os.name == "nt":
        return ["aidb_native.pyd", "aidb_native.dll", "libaidb_native.dll"]
    return ["libaidb_native.so", "aidb_native.so", "aidb_native.abi3.so"]


def dest_name() -> str:
    if os.name == "nt":
        return "aidb_native.pyd"
    return "aidb_native.abi3.so"


def target_roots() -> list[Path]:
    roots: list[Path] = []
    if os.environ.get("CARGO_TARGET_DIR"):
        roots.append(Path(os.environ["CARGO_TARGET_DIR"]))
    roots.append(REPO / "target")
    return roots


def find_artifact(profile: str) -> Path | None:
    names = cargo_names()
    for root in target_roots():
        folder = root / profile
        for name in names:
            candidate = folder / name
            if candidate.exists():
                return candidate
    return None


def resolve_profile(want_release: bool) -> str:
    if want_release:
        return "release"
    env = os.environ.get("AIDB_NATIVE_PROFILE")
    if env in {"debug", "release"}:
        return env
    if find_artifact("release"):
        return "release"
    if find_artifact("debug"):
        return "debug"
    return "release"


def cargo_build(profile: str) -> None:
    args = ["cargo", "build", "-p", "aidb-python"]
    if profile == "release":
        args.append("--release")
    subprocess.check_call(args, cwd=REPO)


def stage(want_release: bool = False) -> Path:
    profile = resolve_profile(want_release)
    source = find_artifact(profile)
    if source is None:
        cargo_build(profile)
        source = find_artifact(profile)
    if source is None:
        raise FileNotFoundError("aidb-python build did not produce a cdylib")
    AIDB_PKG.mkdir(parents=True, exist_ok=True)
    dest = AIDB_PKG / dest_name()
    shutil.copy2(source, dest)
    print(f"staged {source} -> {dest}")
    return dest


def main() -> None:
    want_release = "--release" in sys.argv
    stage(want_release=want_release)


if __name__ == "__main__":
    main()
