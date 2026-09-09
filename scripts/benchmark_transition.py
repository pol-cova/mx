#!/usr/bin/env python3
"""Benchmark one complete profile restore/apply cycle with the release binary."""
import argparse
import json
import os
import subprocess
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path


def run_profile(binary, environment, device, operation):
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as request:
        json.dump({"device": device, "operation": operation, "preset": "slim"}, request)
        request_path = request.name
    started = time.monotonic_ns()
    try:
        result = subprocess.run(
            [str(binary), "profile", request_path],
            env=environment,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"profile {operation} failed ({result.returncode}): "
                f"{result.stderr.strip() or result.stdout.strip()}"
            )
    finally:
        Path(request_path).unlink(missing_ok=True)
    return {
        "wall_time_ms": (time.monotonic_ns() - started) // 1_000_000,
        "result": json.loads(result.stdout),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mx", default="target/release/mx", type=Path)
    parser.add_argument("--state-dir", required=True, type=Path)
    parser.add_argument("--device", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--label", required=True)
    args = parser.parse_args()

    environment = os.environ.copy()
    environment["MX_STATE_DIR"] = str(args.state_dir.resolve())
    report = {
        "label": args.label,
        "captured_at": datetime.now(timezone.utc).isoformat(),
        "binary": str(args.mx.resolve()),
        "device": args.device,
        "restore": run_profile(args.mx.resolve(), environment, args.device, "restore"),
        "apply": run_profile(args.mx.resolve(), environment, args.device, "apply"),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
