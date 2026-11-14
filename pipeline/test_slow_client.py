"""Bounded localhost load regression: a non-reading SSE client must not stop speech display.

Run after cargo build. No band, API keys, sponsor requests or throughput claims.
"""
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
with socket.socket() as reservation:
    reservation.bind(('127.0.0.1', 0))
    port = reservation.getsockname()[1]
process = subprocess.Popen([str(ROOT / 'target/debug/decision-maker'), '--typed'], cwd=ROOT,
                           env=dict(os.environ, BRIDGE_PORT=str(port)),
                           stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
progress = {'turns': 0, 'off': False, 'error': None}
finished = threading.Event()
connection = None
slow = None
try:
    deadline = time.monotonic() + 5
    while True:
        try:
            connection = http.client.HTTPConnection('127.0.0.1', port, timeout=20)
            connection.request('GET', '/events')
            response = connection.getresponse()
            assert response.status == 200
            break
        except OSError:
            assert process.poll() is None, process.stderr.read()
            assert time.monotonic() < deadline
            time.sleep(0.02)

    def read():
        try:
            for line in response:
                if line.startswith(b'data: '):
                    event = json.loads(line[6:])
                    if event['type'] == 'gap':
                        raise AssertionError(event['message'])
                    if event['type'] == 'transcript' and event['final']:
                        progress['turns'] += 1
                    if event['type'] == 'mode' and not event['decisions'] and event['conversation'] == 1:
                        progress['off'] = True
                        finished.set()
        except Exception as e:
            progress['error'] = str(e)
            finished.set()

    threading.Thread(target=read, daemon=True).start()
    slow = socket.socket()
    slow.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
    slow.settimeout(2)
    slow.connect(('127.0.0.1', port))
    slow.sendall(f'GET /events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n'.encode())
    headers = b''
    while not headers.endswith(b'\r\n\r\n'):
        headers += slow.recv(1)
    assert headers.startswith(b'HTTP/1.1 200')
    # Leave this subscriber unread while sending more than typical socket buffers hold.
    process.stdin.write('/triple\n')
    process.stdin.flush()
    for i in range(800):
        process.stdin.write(f'{i}: ' + 'x' * 7900 + '\n')
        process.stdin.flush()
        time.sleep(1 / 80)
    process.stdin.write('/triple\n')
    process.stdin.flush()
    assert finished.wait(10), progress
    assert progress == {'turns': 800, 'off': True, 'error': None}, progress
    print('Slow SSE reader: all 800 large final turns reached the healthy subscriber; capture toggled off.')
finally:
    if slow:
        slow.close()
    if connection:
        connection.close()
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
