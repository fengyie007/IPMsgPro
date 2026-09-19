"""End-to-end smoke test: a CLI test runner sends a text and a file to a GUI instance.

Usage: python tools/e2e_smoke.py <exe> <gui_port> <cli_port>
Starts a GUI instance (--verbose) on <gui_port>, runs `--mode=cli --cmd=test`
on <cli_port> against it, then prints the relevant log lines and the GUI
instance's database rows. Both instances are killed at the end.
"""
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time

# Log lines contain UTF-8 (Chinese, "✓"); the Windows console may be GBK.
sys.stdout.reconfigure(encoding='utf-8', errors='replace')

exe, gui_port, cli_port = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
home = os.environ['USERPROFILE']
gui_dir = os.path.join(home, '.speedipmsg_%d' % gui_port)
cli_dir = os.path.join(home, '.speedipmsg_%d' % cli_port)

subprocess.run(['taskkill', '/F', '/IM', os.path.basename(exe)], capture_output=True)
time.sleep(1.5)
for d in (gui_dir, cli_dir):
    shutil.rmtree(d, ignore_errors=True)

payload = os.path.join(os.environ['TEMP'], 'e2e_payload_中文.txt')
with open(payload, 'wb') as f:
    f.write(('hello from cli 你好\n' * 2000).encode('utf-8'))   # ~60 KB
cfg = os.path.join(os.environ['TEMP'], 'e2e_test.json')
with open(cfg, 'w', encoding='utf-8') as f:
    json.dump({
        'target_ip': '127.0.0.1', 'target_port': gui_port,
        'tests': [
            {'type': 'text', 'content': 'e2e 文本消息 ✓', 'delay_ms': 800},
            {'type': 'file', 'content': payload, 'delay_ms': 800},
        ],
    }, f, ensure_ascii=False)

gui = subprocess.Popen([exe, '--port=%d' % gui_port, '--verbose'])
time.sleep(7)
cli = subprocess.Popen([exe, '--mode=cli', '--cmd=test', '--port=%d' % cli_port,
                        '--target=127.0.0.1:%d' % gui_port, '--config=%s' % cfg])
cli.wait(timeout=60)
time.sleep(2)
subprocess.run(['taskkill', '/F', '/IM', os.path.basename(exe)], capture_output=True)
time.sleep(1.5)


def show(path, keys, limit=25):
    n = 0
    with open(path, encoding='utf-8', errors='replace') as f:
        for line in f:
            if any(k in line for k in keys):
                print('   ', line.rstrip()[:200])
                n += 1
                if n >= limit:
                    break


print('=== CLI runner log ===')
show(os.path.join(cli_dir, 'ipmsg_gui_debug.log'), ['[CLI]', 'CRASH', '[ERROR]'])
print('=== GUI log: receive path ===')
show(os.path.join(gui_dir, 'ipmsg_gui_debug.log'),
     ['USER DISCOVERED', 'FILE_REQ_EMIT', 'receive_request', 'Log level', 'CRASH', '[ERROR]', '[WARN]',
      'GUI-MSG] body', 'SendMessage to'])
print('=== GUI database rows ===')
db = os.path.join(gui_dir, 'ipmsg.db')
con = sqlite3.connect('file:%s?mode=ro' % db.replace('\\', '/'), uri=True)
for r in con.execute('SELECT id, from_id, to_id, content, type, status FROM messages'):
    print('   ', r)
log_lines = list(open(os.path.join(gui_dir, 'ipmsg_gui_debug.log'), encoding='utf-8', errors='replace'))
print('=== GUI log size: %d lines, DEBUG lines: %d ===' % (
    len(log_lines), sum(1 for l in log_lines if '[DEBUG]' in l)))
