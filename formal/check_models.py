"""Check Maude searches, including their expected non-vacuous witnesses."""

from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
CASES = (
    ("check.maude", 1, 4),
    ("check_two_task.maude", 2, 9),
    ("check_reply_capability.maude", 1, 10),
)


def main() -> int:
    for filename, no_solutions, witnesses in CASES:
        try:
            result = subprocess.run(
                ["maude", "-no-banner", str(ROOT / "formal" / filename)],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
        except FileNotFoundError:
            print("Maude is required to run the formal checks", file=sys.stderr)
            return 1

        output = result.stdout + result.stderr
        errors = []
        if result.returncode != 0:
            errors.append(f"Maude exited {result.returncode}")
        if "Warning:" in output or "Error:" in output:
            errors.append("Maude reported a warning or error")
        if "result Bool: true" not in output:
            errors.append("bad-state sanity check did not return true")
        if output.count("No solution.") != no_solutions:
            errors.append("unexpected number of unreachable searches")
        if output.count("Solution 1") != witnesses:
            errors.append("a required behavior witness was not found")

        if errors:
            print(f"{filename}: FAIL: {', '.join(errors)}", file=sys.stderr)
            print(output, file=sys.stderr)
            return 1
        print(f"{filename}: PASS ({no_solutions} unreachable, {witnesses} witnesses)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
