#!/usr/bin/env python3
"""Measure fresh Phoebe builds and standalone process runs separately."""

import argparse
import platform
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def measured(command: list[str], expected: str) -> float:
    start = time.perf_counter_ns()
    result = subprocess.run(command, capture_output=True, text=True)
    elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
    if result.returncode:
        raise RuntimeError(
            f"{command[0]} exited {result.returncode}: {result.stderr.strip()}"
        )
    if result.stdout.strip() != expected:
        raise RuntimeError(
            f"{command[0]} returned {result.stdout!r}, expected {expected!r}"
        )
    return elapsed_ms


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("expected", help="expected stdout value from main")
    parser.add_argument("--compiler", type=Path, default=Path("target/release/phoebe"))
    parser.add_argument("--builds", type=int, default=20)
    parser.add_argument("--runs", type=int, default=30)
    args = parser.parse_args()
    if args.builds < 1 or args.runs < 1:
        parser.error("--builds and --runs must be positive")
    compiler = args.compiler.resolve(strict=True)
    source = args.source.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="phoebe-bench-") as directory:
        output = Path(directory) / "program"
        build_times = []
        for _ in range(args.builds):
            build_times.append(
                measured(
                    [str(compiler), "build", str(source), "-o", str(output)],
                    f"built {output}",
                )
            )
        for _ in range(3):
            measured([str(output)], args.expected)
        run_times = [measured([str(output)], args.expected) for _ in range(args.runs)]
    print(f"host: {platform.system().lower()}-{platform.machine().lower()}")
    print(f"source: {source}")
    print(f"builds: {args.builds}, runs: {args.runs} (after 3 warmups)")
    print(f"median fresh build: {statistics.median(build_times):.3f} ms")
    print(f"median standalone run: {statistics.median(run_times):.3f} ms")


if __name__ == "__main__":
    main()
