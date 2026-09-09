#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
from experiment_contract import REQUIRED_PROBES, distribution, save, validate_probe_results

STATE_ORDER = (
    'baseline_before',
    'candidate_idle',
    'candidate_active',
    'candidate_post_interaction',
    'baseline_restored',
)


def validate(report):
    manifest = report['manifest']
    if manifest['schema_version'] != 1:
        raise ValueError('unsupported experiment schema version')
    if manifest['sample_count'] != 30 or manifest['cycle_count'] != 3:
        raise ValueError('accepted experiments require 30 samples and three cycles')
    cycles = report['cycles']
    if len(cycles) != manifest['cycle_count']:
        raise ValueError('experiment has an incomplete cycle set')
    for expected_index, cycle in enumerate(cycles, 1):
        if cycle['index'] != expected_index:
            raise ValueError('cycle indexes must be contiguous and start at one')
        states = [measurement['state'] for measurement in cycle['measurements']]
        if states != list(STATE_ORDER):
            raise ValueError(f'cycle {expected_index} does not follow A/B/A state order')
        for measurement in cycle['measurements']:
            if len(measurement['samples']) != manifest['sample_count']:
                raise ValueError(f'cycle {expected_index} {measurement["state"]} has the wrong sample count')
            measurement['physical_bytes'] = distribution([sample['physical_bytes'] for sample in measurement['samples']])
            measurement['process_count'] = distribution([sample['process_count'] for sample in measurement['samples']])
        validate_probe_results(cycle['probes'])
        before = cycle['measurements'][0]['physical_bytes']['median']
        restored = cycle['measurements'][-1]['physical_bytes']['median']
        delta = 0 if before == restored == 0 else abs(restored - before) / before
        cycle['restoration_delta'] = delta
        if delta > 0.05:
            raise ValueError(f'cycle {expected_index} restored baseline differs by more than 5%')
    report['required_probes'] = list(REQUIRED_PROBES)
    report['accepted'] = True
    return report


def main():
    parser = argparse.ArgumentParser(description='Validate and summarize a completed Mx A/B/A experiment.')
    parser.add_argument('--input', required=True)
    parser.add_argument('--output')
    args = parser.parse_args()
    source = Path(args.input)
    report = validate(json.loads(source.read_text()))
    destination = Path(args.output) if args.output else source
    save(destination, report)
    print(f'Accepted experiment evidence: {destination}')


if __name__ == '__main__':
    main()
