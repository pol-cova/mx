#!/usr/bin/env python3
"""Benchmark repeated warm Mx navigation-flow captures with a prebuilt app."""
import argparse
import json
from pathlib import Path
import statistics
import subprocess
import time


def call(mx, *args):
    started = time.monotonic()
    result = subprocess.run([mx, *args], check=True, text=True, capture_output=True)
    return json.loads(result.stdout), round((time.monotonic() - started) * 1000)


def percentile(values, fraction):
    ordered = sorted(values)
    index = max(0, int(len(ordered) * fraction + 0.999999) - 1)
    return ordered[index]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mx", default="target/release/mx")
    parser.add_argument("--device", required=True)
    parser.add_argument("--app", required=True)
    parser.add_argument("--bundle-id", required=True)
    parser.add_argument("--plan", required=True, help="Flow plan used as a template")
    parser.add_argument("--output", default=".mx/flow-benchmarks")
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--reuse-installed", action="store_true")
    args = parser.parse_args()
    if not 3 <= args.repetitions <= 20:
        parser.error("--repetitions must be 3..20")

    root = Path(args.output) / str(int(time.time()))
    root.mkdir(parents=True)
    template = json.loads(Path(args.plan).read_text())
    samples = []
    for index in range(1, args.repetitions + 1):
        if args.reuse_installed:
            launch, launch_wall_ms = call(args.mx, "relaunch", "--device", args.device)
        else:
            launch, launch_wall_ms = call(args.mx, "launch", "--app", args.app, "--device", args.device)
        plan = dict(template)
        plan["device"] = args.device
        plan["output_dir"] = str((root / f"run-{index}").resolve())
        plan_path = root / f"plan-{index}.json"
        plan_path.write_text(json.dumps(plan, indent=2) + "\n")
        flow, flow_wall_ms = call(args.mx, "capture-flow", str(plan_path))
        call(args.mx, "stop", "--device", args.device, "--bundle-id", args.bundle_id)
        states = flow["states"]
        samples.append({
            "run": index,
            "launch_wall_ms": launch_wall_ms,
            "launch_reported_ms": launch["elapsed_ms"],
            "flow_wall_ms": flow_wall_ms,
            "flow_reported_ms": flow["elapsed_ms"],
            "transition_ms": sum(state["transition_ms"] for state in states),
            "screenshot_ms": sum(state["screenshot_ms"] for state in states),
            "viewer": flow["viewer"],
        })
    report = {
        "repetitions": args.repetitions,
        "screens_per_run": len(template["states"]),
        "median_launch_ms": statistics.median(s["launch_wall_ms"] for s in samples),
        "median_flow_ms": statistics.median(s["flow_wall_ms"] for s in samples),
        "median_transition_ms": statistics.median(s["transition_ms"] for s in samples),
        "median_screenshot_ms": statistics.median(s["screenshot_ms"] for s in samples),
        "p95_launch_ms": percentile([s["launch_wall_ms"] for s in samples], 0.95),
        "p95_flow_ms": percentile([s["flow_wall_ms"] for s in samples], 0.95),
        "max_flow_ms": max(s["flow_wall_ms"] for s in samples),
        "p95_transition_ms": percentile([s["transition_ms"] for s in samples], 0.95),
        "p95_screenshot_ms": percentile([s["screenshot_ms"] for s in samples], 0.95),
        "samples": samples,
    }
    report_path = root / "benchmark.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({**report, "report": str(report_path.resolve())}, indent=2))


if __name__ == "__main__":
    main()
