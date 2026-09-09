"""Small stdio MCP client shared by integration and profiling scripts."""
import json
import queue
import subprocess
import threading
import time

class Client:
    def __init__(self, binary, stderr=None, env=None):
        self.process = subprocess.Popen([str(binary), 'mcp'], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=stderr, text=True, bufsize=1, env=env)
        self.messages = queue.Queue()
        self.records = []
        self.notifications = []
        self.sequence = 0
        def reader():
            try:
                for line in self.process.stdout:
                    self.messages.put((json.loads(line), len(line.encode())))
            except Exception as error:
                self.messages.put(({'reader_error': str(error)}, 0))
            finally:
                self.messages.put((None, 0))
        threading.Thread(target=reader, daemon=True).start()
        self.request('initialize', {'protocolVersion':'2025-06-18','capabilities':{},
            'clientInfo':{'name':'mx-profiler','version':'1'}})
        self.send({'jsonrpc':'2.0','method':'notifications/initialized'})

    def send(self, value):
        payload = json.dumps(value, ensure_ascii=False) + '\n'
        self.process.stdin.write(payload)
        self.process.stdin.flush()
        return len(payload.encode())

    def request(self, method, params):
        self.sequence += 1
        started = time.monotonic()
        sent = self.send({'jsonrpc':'2.0','id':self.sequence,'method':method,'params':params})
        received = 0
        while True:
            message, size = self.messages.get(timeout=max(0.01, 750 - (time.monotonic()-started)))
            received += size
            if message is None or 'reader_error' in message:
                raise RuntimeError(f'MCP stream closed or malformed: {message}')
            if 'id' not in message:
                self.notifications.append(message)
                continue
            if message['id'] != self.sequence:
                raise RuntimeError(f'Unexpected MCP response: {message}')
            self.records.append({'method':params.get('name',method), 'elapsed_s':time.monotonic()-started,
                'request_bytes':sent,'response_bytes':received, 'at':started})
            if 'error' in message:
                raise RuntimeError(message['error'])
            return message['result']

    def call(self, name, arguments):
        result = self.request('tools/call', {'name':name,'arguments':arguments})
        if result.get('isError'):
            raise RuntimeError(result['content'])
        return result.get('structuredContent', result)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=5)
