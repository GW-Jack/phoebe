"""Check Maude searches, including their expected non-vacuous witnesses."""

from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
CASES = (
    ("check.maude", 1, 4, 1),
    ("check_two_task.maude", 2, 9, 1),
    ("check_reply_capability.maude", 1, 10, 1),
    ("check_request_backpressure.maude", 1, 7, 1),
    ("check_retained_borrow.maude", 1, 4, 1),
    ("check_lexical_scopes.maude", 2, 7, 1),
    ("check_product_capabilities.maude", 2, 12, 1),
    ("check_sum_capabilities.maude", 4, 22, 3),
    ("check_mutable_right_places.maude", 3, 11, 2),
)


def main() -> int:
    for filename, no_solutions, witnesses, bad_state_sanity_checks in CASES:
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
        if output.count("result Bool: true") != bad_state_sanity_checks:
            errors.append("unexpected number of true bad-state sanity checks")
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
