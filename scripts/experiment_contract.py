#!/usr/bin/env python3
import hashlib
import json
import math
import platform
import statistics
import subprocess
from pathlib import Path

SCHEMA_VERSION = 1
REQUIRED_PROBES = (
    'build',
    'install',
    'launch',
    'semantic_inspection',
    'unicode_input',
    'assertion',
    'screenshot',
    'logs',
    'networking',
)


def sha256(path):
    value = Path(path)
    return hashlib.sha256(value.read_bytes()).hexdigest()


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def environment(runtime='iOS 26.5', device_type='iPhone 17 Pro'):
    return {
        'macos': command('sw_vers'),
        'architecture': platform.machine(),
        'xcode': command('xcodebuild', '-version'),
        'runtime': runtime,
        'device_type': device_type,
    }


def source_identity(root='.'):
    result = subprocess.run(['git', 'rev-parse', '--verify', 'HEAD'], cwd=root, text=True, capture_output=True)
    if result.returncode == 0:
        return result.stdout.strip()
    status = subprocess.check_output(['git', 'status', '--short', '--untracked-files=all'], cwd=root)
    return 'unborn-' + hashlib.sha256(status).hexdigest()


def manifest(experiment_id, candidate_id, mx, catalog, task_script, app_build_identity, runtime='iOS 26.5', device_type='iPhone 17 Pro', sample_count=30, cycle_count=3, source_revision=None):
    return {
        'schema_version': SCHEMA_VERSION,
        'experiment_id': experiment_id,
        'candidate_id': candidate_id,
        'source_revision': source_revision or source_identity(),
        'mx_sha256': sha256(mx),
        'catalog_sha256': sha256(catalog),
        'app_build_identity': app_build_identity,
        'task_script_sha256': sha256(task_script),
        'sample_count': sample_count,
        'cycle_count': cycle_count,
        'environment': environment(runtime, device_type),
    }


def distribution(values):
    ordered = sorted(values)
    if not ordered or any(not math.isfinite(value) for value in ordered):
        raise ValueError('samples must contain finite values')
    return {
        'count': len(ordered),
        'minimum': ordered[0],
        'median': statistics.median(ordered),
        'p95_nearest_rank': ordered[math.ceil(0.95 * len(ordered)) - 1],
        'maximum': ordered[-1],
    }


def host_snapshot():
    return {
        'sysctl': command('/usr/sbin/sysctl', 'hw.memsize', 'vm.swapusage', 'kern.memorystatus_vm_pressure_level'),
        'vm_stat': command('/usr/bin/vm_stat'),
    }


def save(path, report):
    Path(path).write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')


def validate_probe_results(results):
    missing = [probe for probe in REQUIRED_PROBES if probe not in results]
    failed = [probe for probe in REQUIRED_PROBES if probe in results and not results[probe].get('passed')]
    if missing:
        raise ValueError('missing required probes: ' + ', '.join(missing))
    if failed:
        raise ValueError('failed required probes: ' + ', '.join(failed))
