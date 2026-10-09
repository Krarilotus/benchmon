"""Read one sample from every configured machine without opening the GUI.

Requires Python 3 locally. Uses hosts.txt (ignored by Git); falls back to this PC only.
Stops all owned diagnostic processes when finished. Never prints SSH credentials.
"""
import base64
import concurrent.futures
import json
from pathlib import Path
import subprocess
import threading
import sys

ROOT = Path(__file__).resolve().parent.parent


def read_sample(name, platform, destination):
    if platform in ('local', 'windows'):
        source = (ROOT / 'src/collectors/windows.ps1').read_text(encoding='utf-8')
        encoded = base64.b64encode(source.encode('utf-16-le')).decode()
        command = ['powershell', '-NoProfile', '-NonInteractive', '-EncodedCommand', encoded]
        if platform == 'windows':
            command = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', '-o',
                       'StrictHostKeyChecking=yes', destination, ' '.join(command)]
        data = None
    elif platform == 'linux':
        command = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', '-o',
                   'StrictHostKeyChecking=yes', destination, 'python3 -u -']
        data = (ROOT / 'src/collectors/linux.py').read_bytes()
    else:
        raise ValueError('Unknown platform')
    process = subprocess.Popen(command, stdin=subprocess.PIPE if data else subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    timed_out = threading.Event()

    def expire():
        timed_out.set()
        process.kill()

    timer = threading.Timer(25, expire)
    timer.start()
    try:
        if data:
            process.stdin.write(data)
            process.stdin.close()
        for raw in process.stdout:
            line = raw.decode('utf-8', 'replace').strip()
            if not line.startswith('S '):
                continue
            metrics, separator, disks = line.partition(' | D ')
            if not separator:
                raise ValueError('Disk suffix missing')
            disks = json.loads(disks)
            if not isinstance(disks, list) or not disks:
                raise ValueError('No local disk readings')
            fields = metrics.split(maxsplit=4)
            cpu = float(fields[1].replace(',', '.'))
            summary = []
            for disk in disks:
                total, used = disk['total_bytes'], disk['used_bytes']
                if not 0 <= used <= total or total <= 0:
                    raise ValueError('Invalid disk capacity')
                read, write = disk.get('read_percent'), disk.get('write_percent')
                summary.append(dict(name=disk['name'], used_percent=round(100 * used / total, 1),
                                    total_gib=round(total / 1024**3, 1),
                                    io_percent=round(min(100, max(read, write)), 1)
                                    if read is not None and write is not None else None))
            return dict(name=name, status='ok', cpu_percent=cpu, disks=summary)
        raise RuntimeError('Timed out' if timed_out.is_set() else 'Probe ended before a sample')
    finally:
        timer.cancel()
        if process.poll() is None:
            process.kill()
        process.wait()


def main():
    config = ROOT / 'hosts.txt'
    text = config.read_text(encoding='utf-8-sig') if config.exists() else 'This PC|local'
    hosts = []
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        fields = [field.strip() for field in line.split('|')]
        if len(fields) not in (2, 3) or (len(fields) == 3 and fields[2].startswith('-')):
            raise ValueError('Invalid host configuration')
        hosts.append((fields[0], fields[1], fields[2] if len(fields) == 3 else None))
    failed = False
    with concurrent.futures.ThreadPoolExecutor(max_workers=min(8, len(hosts) or 1)) as pool:
        futures = {pool.submit(read_sample, *host): host[0] for host in hosts}
        for future in concurrent.futures.as_completed(futures):
            try:
                print(json.dumps(future.result()))
            except Exception as error:
                failed = True
                print(json.dumps(dict(name=futures[future], status='error', message=str(error))))
    return int(failed)


if __name__ == '__main__':
    sys.exit(main())
