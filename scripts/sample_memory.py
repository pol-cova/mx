#!/usr/bin/env python3
import argparse
import json
import statistics
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mx", default="target/release/mx")
    parser.add_argument("--device", required=True)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--interval", type=float, default=1.0)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.samples <= 100:
        parser.error("--samples must be 1..100")

    samples = []
    for index in range(args.samples):
        result = subprocess.run(
            [args.mx, "metrics", "--device", args.device],
            check=True,
            capture_output=True,
            text=True,
        )
        value = json.loads(result.stdout)
        samples.append(
            {
                "physical_bytes": value["simulator_physical_bytes"],
                "process_count": value["simulator_process_count"],
                "largest_processes": value["processes"][:12],
            }
        )
        if index + 1 < args.samples:
            time.sleep(args.interval)

    physical = [sample["physical_bytes"] for sample in samples]
    processes = [sample["process_count"] for sample in samples]
    report = {
        "captured_at": datetime.now(timezone.utc).isoformat(),
        "device": args.device,
        "samples": samples,
        "summary": {
            "sample_count": len(samples),
            "median_physical_bytes": statistics.median(physical),
            "min_physical_bytes": min(physical),
            "max_physical_bytes": max(physical),
            "median_process_count": statistics.median(processes),
        },
        "measurement": "proc_pid_rusage v4 physical footprint summed over descendants of device launchd_sim",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
