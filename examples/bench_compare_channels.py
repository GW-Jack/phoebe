#!/usr/bin/env python3
"""Compare scalar and nominal typed one-slot channel executables."""

import argparse
import platform
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parent
FIXTURES = {
    "scalar": ROOT / "bench_channel_throughput.phoebe",
    "typed": ROOT / "bench_typed_channel_throughput.phoebe",
}


def checked(command: list[str], expected: str) -> float:
    started = time.perf_counter_ns()
    result = subprocess.run(command, capture_output=True, text=True, check=True)
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    if result.stdout.strip() != expected:
        raise RuntimeError(f"{command[0]} returned {result.stdout!r}, expected {expected!r}")
    return elapsed_ms


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, default=Path("target/release/phoebe"))
    parser.add_argument("--messages", type=int, default=1_000_000)
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--warmups", type=int, default=3)
    args = parser.parse_args()
    if not (1 <= args.messages <= 4_000_000_000):
        parser.error("--messages must be between 1 and 4,000,000,000")
    if args.runs < 1 or args.warmups < 0:
        parser.error("--runs must be positive and --warmups nonnegative")
    compiler = args.compiler.resolve(strict=True)
    expected = str(args.messages * (args.messages - 1) // 2)
    with tempfile.TemporaryDirectory(prefix="phoebe-channel-compare-") as directory:
        outputs = {}
        for kind, fixture in FIXTURES.items():
            source = Path(directory) / f"{kind}.phoebe"
            template = fixture.read_text()
            if template.count("10000") != 3:
                raise RuntimeError(f"unexpected message-count sites in {fixture}")
            source.write_text(template.replace("10000", str(args.messages)))
            output = Path(directory) / kind
            checked(
                [str(compiler), "build", str(source), "-o", str(output)],
                f"built {output}",
            )
            outputs[kind] = output

        def run(kind: str) -> float:
            return checked([str(outputs[kind])], expected)

        for _ in range(args.warmups):
            for kind in outputs:
                run(kind)
        samples = {kind: [] for kind in outputs}
        for index in range(args.runs):
            order = ("scalar", "typed") if index % 2 == 0 else ("typed", "scalar")
            for kind in order:
                samples[kind].append(run(kind))

    print(f"host: {platform.system().lower()}-{platform.machine().lower()}")
    print(f"messages: {args.messages}, runs per program: {args.runs}")
    for kind, values in samples.items():
        print(
            f"{kind}: median {statistics.median(values):.3f} ms, "
            f"range {min(values):.3f}–{max(values):.3f} ms"
        )


if __name__ == "__main__":
    main()
