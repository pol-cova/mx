#!/usr/bin/env python3
"""Exercise Mx's v0.1 workflow over MCP, without screenshots or computer use."""
import argparse
import json
from pathlib import Path
import queue
import subprocess
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--device', required=True, help='iOS simulator UDID')
    parser.add_argument('--mx', default='target/debug/mx')
    parser.add_argument('--report', default='.mx/demo-report.json')
    parser.add_argument('--screenshot', help='Also test the explicit PNG fallback at this new path')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    report = {'checks': [], 'screenshots': 0, 'vision_calls': 0, 'coordinate_actions': 0, 'mcp_requests': 0}
    started = time.monotonic()
    messages = queue.Queue()
    with subprocess.Popen([str(Path(args.mx).resolve()), 'mcp'], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, text=True, bufsize=1) as server:
        def read_messages():
            for line in server.stdout:
                messages.put(json.loads(line))
            messages.put(None)
        threading.Thread(target=read_messages, daemon=True).start()
        request_id = 0

        def send(message):
            server.stdin.write(json.dumps(message) + '\n')
            server.stdin.flush()

        def request(method, params):
            nonlocal request_id
            request_id += 1
            report['mcp_requests'] += 1
            send({'jsonrpc': '2.0', 'id': request_id, 'method': method, 'params': params})
            deadline = time.monotonic() + 750
            while True:
                message = messages.get(timeout=max(0.01, deadline - time.monotonic()))
                if message is None:
                    raise RuntimeError('Mx closed its protocol stream')
                if message.get('id') != request_id:
                    continue
                if 'error' in message:
                    raise RuntimeError(message['error'])
                return message['result']

        def call(name, arguments):
            result = request('tools/call', {'name': name, 'arguments': arguments})
            if result.get('isError'):
                raise RuntimeError(result['content'])
            return result['structuredContent']

        def expect_label(label):
            deadline = time.monotonic() + 15
            while True:
                screen = call('mx_ui', {'device': args.device})
                if any(e.get('label') == label for e in screen['elements']):
                    report['checks'].append(label)
                    return screen
                if time.monotonic() >= deadline:
                    raise AssertionError(f'{label!r} missing in {screen}')
                time.sleep(0.25)

        launched = False
        try:
            request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                   'clientInfo': {'name': 'mx-demo', 'version': '1'}})
            send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
            report['tool_count'] = len(request('tools/list', {})['tools'])
            report['server_idle_rss_kib'] = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(server.pid)], text=True).strip())
            print('Building and launching MxDemo through MCP...', flush=True)
            report['run'] = call('mx_run', {'project': str(root / 'examples/MxDemo'),
                                            'scheme': 'MxDemo', 'device': args.device})
            launched = True
            expect_label('Count: 0')
            call('mx_tap', {'device': args.device, 'selector': {'identifier': 'increment'}})
            expect_label('Count: 1')
            call('mx_tap', {'device': args.device, 'selector': {'identifier': 'name'}})
            call('mx_type', {'device': args.device, 'text': 'Mx'})
            call('mx_tap', {'device': args.device, 'selector': {'identifier': 'greet'}})
            report['final_ui'] = expect_label('Hello, Mx!')
            report['logs'] = call('mx_logs', {'device': args.device, 'pid': report['run']['pid'], 'seconds': 60})
            if args.screenshot:
                screenshot = call('mx_screenshot', {'device': args.device, 'output': str(Path(args.screenshot).resolve())})
                assert Path(screenshot['path']).read_bytes().startswith(b'\x89PNG\r\n\x1a\n')
                report['screenshots'] = 1
                report['screenshot'] = screenshot
            report['passed'] = True
        finally:
            if launched:
                call('mx_stop', {'device': args.device, 'bundle_id': 'dev.mx.demo'})
            server.stdin.close()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.terminate()
                server.wait(timeout=5)
    report['elapsed_seconds'] = round(time.monotonic() - started, 2)
    path = Path(args.report)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Passed: {', '.join(report['checks'])}. Report: {path}")


if __name__ == '__main__':
    main()
