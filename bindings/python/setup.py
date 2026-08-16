"""Build a platform wheel that includes the PyO3 module. No hand-copied dylib."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py
from setuptools.dist import Distribution


class BinaryDistribution(Distribution):
    def has_ext_modules(self) -> bool:
        return True


class BuildPy(build_py):
    def run(self) -> None:
        script = Path(__file__).resolve().parent / "scripts" / "stage_native.py"
        subprocess.check_call([sys.executable, str(script)])
        super().run()


setup(distclass=BinaryDistribution, cmdclass={"build_py": BuildPy})
