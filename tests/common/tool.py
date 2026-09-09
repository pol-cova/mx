#!/usr/bin/python3
import json, os, pathlib, plistlib, sys
root = pathlib.Path(os.environ['MX_TEST_ROOT'])
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as f:
    f.write(json.dumps({'program': name, 'args': args}) + '\n')
if name == 'xcodebuild':
    if '-list' in args:
        print(json.dumps({'project': {'schemes': ['Demo']}}))
    elif '-showBuildSettings' in args:
        print(json.dumps([{'buildSettings': {'PRODUCT_TYPE': 'com.apple.product-type.application', 'TARGET_BUILD_DIR': str(root / 'Build Products'), 'FULL_PRODUCT_NAME': 'Demo.app'}}]))
    else:
        if os.environ.get('MX_TEST_SLOW_BUILD') and not any('test-device-2' in a for a in args):
            import time
            (root / 'build.pid').write_text(str(os.getpid()))
            time.sleep(30)
        if os.environ.get('MX_TEST_FAIL_BUILD'):
            print('App.swift:7:1: error: intentional build failure', file=sys.stderr)
            sys.exit(65)
        app = root / 'Build Products/Demo.app'
        app.mkdir(parents=True, exist_ok=True)
        (app / 'Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier': 'dev.mx.test'}))
        print('BUILD SUCCEEDED')
elif name == 'xcrun':
    op = args[1]
    if op == 'list':
        devices = [{'name':'Test Phone','udid':'test-device','state':'Booted' if os.environ.get('MX_TEST_BOOTED') or (root/'booted').exists() else 'Shutdown','isAvailable':True,'deviceTypeIdentifier':'com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro'}]
        if os.environ.get('MX_TEST_TWO_DEVICES'):
            devices.append({'name':'Test Phone 2','udid':'test-device-2','state':'Shutdown','isAvailable':True,'deviceTypeIdentifier':'com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro'})
        print(json.dumps({'devices':{'com.apple.CoreSimulator.SimRuntime.iOS-26-5':devices}}))
    elif op == 'create':
        print('00000000-0000-4000-8000-000000000001')
    elif op == 'boot':
        (root/'booted').touch()
    elif op == 'pbcopy':
        (root / 'pasteboard.txt').write_text(sys.stdin.read())
    elif op == 'launch':
        print('dev.mx.test: 4321')
    elif op == 'spawn' and args[3] == '/bin/sh':
        import re
        script = args[5]
        if ' bootout "user/501/$label"' in script:
            sys.exit(0)
        transition = re.search(r'bin/launchctl" (disable|enable) "system/\$label"', script)
        label_list = re.search(r'for label in(.*); do', script)
        if transition is None or label_list is None:
            print('invalid batch transition script', file=sys.stderr)
            sys.exit(2)
        labels = label_list.group(1).split()
        path = root/'disabled.json'
        disabled = set(json.loads(path.read_text())) if path.exists() else set()
        action = transition.group(1)
        for index, label in enumerate(labels):
            if os.environ.get('MX_TEST_PROFILE_FAIL') and index >= 3:
                path.write_text(json.dumps(sorted(disabled)))
                print('injected batch transition failure', file=sys.stderr)
                sys.exit(1)
            if action == 'disable': disabled.add(label)
            else: disabled.discard(label)
        path.write_text(json.dumps(sorted(disabled)))
    elif op == 'spawn' and args[3] == 'launchctl':
        import fcntl
        lock = (root/'disabled.lock').open('w')
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = root/'disabled.json'
        labels = set(json.loads(path.read_text())) if path.exists() else set()
        action = args[4]
        if action == 'print-disabled':
            for label in sorted(labels):print(f'"{label}" => disabled')
        else:
            label = args[5].removeprefix('system/')
            if action == 'disable':
                if os.environ.get('MX_TEST_PROFILE_FAIL') and len(labels) >= 3:
                    print('injected service transition failure',file=sys.stderr);sys.exit(1)
                labels.add(label)
            elif action == 'enable':labels.discard(label)
            path.write_text(json.dumps(sorted(labels)))
        fcntl.flock(lock, fcntl.LOCK_UN)
        lock.close()
    elif op == 'spawn':
        for i in range(250):
            print(json.dumps({'eventMessage': 'line ' + str(i)}))
    elif op == 'io':
        if pathlib.Path(args[-1]).exists():
            print('simctl requires a new output file', file=sys.stderr)
            sys.exit(1)
        pathlib.Path(args[-1]).write_bytes(b'\x89PNG\r\n\x1a\n')
elif name == 'axe':
    if args[0] == 'describe-ui':
        print(json.dumps([{'type': 'Application', 'pid': int(os.environ.get('MX_TEST_FOREGROUND_PID','4321')), 'children': [
            {'type': 'Button', 'AXLabel': 'Continue', 'AXUniqueId': 'first'},
            {'type': 'Button', 'AXLabel': 'Continue', 'AXUniqueId': 'second'},
            {'type': 'TextField', 'AXLabel': 'Name', 'AXUniqueId': 'name', 'AXValue': ''}
        ]}]))
    elif args[0] == 'key-combo':
        (root / 'typed.txt').write_text((root / 'pasteboard.txt').read_text())
    elif args[0] == 'type':
        (root / 'typed.txt').write_text(sys.stdin.read())
