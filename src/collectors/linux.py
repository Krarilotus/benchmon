"""Stream Linux metrics using the standard library and optional nvidia-smi.

Disk capacity is counted once per mounted block filesystem (bind mounts are deduplicated).
Read/write busy times come from /proc/diskstats, sampled over the same interval as CPU.
"""
import json
import os
import re
import shutil
import subprocess
import time

INTERVAL = 2
CAPACITY_INTERVAL = 30


def cpu_ticks():
    with open('/proc/stat', encoding='ascii') as stream:
        values = [int(value) for value in stream.readline().split()[1:9]]
    return sum(values), values[3] + values[4]


def memory():
    with open('/proc/meminfo', encoding='ascii') as stream:
        fields = {line.split(':')[0]: int(line.split()[1]) for line in stream}
    return fields['MemTotal'] - fields['MemAvailable'], fields['MemTotal']


def disk_ticks():
    ticks = {}
    with open('/proc/diskstats', encoding='ascii') as stream:
        for line in stream:
            fields = line.split()
            if len(fields) >= 14:
                ticks[fields[0] + ':' + fields[1]] = (int(fields[6]), int(fields[10]))
    return ticks


def unescape_mount(value):
    return re.sub(r'\\([0-7]{3})', lambda match: chr(int(match[1], 8)), value)


def disk_capacity():
    volumes = {}
    with open('/proc/self/mountinfo', encoding='utf-8') as stream:
        for line in stream:
            mount, filesystem = line.rstrip().split(' - ', 1)
            fields = mount.split()
            fs_fields = filesystem.split()
            device = fields[2]
            # Exclude pseudo filesystems, network mounts, snapshots and container layers.
            if device.startswith('0:') or fs_fields[0] in ('squashfs', 'overlay'):
                continue
            name = unescape_mount(fields[4])
            if device in volumes and len(volumes[device]['name']) <= len(name):
                continue
            try:
                stat = os.statvfs(name)
            except OSError:
                continue
            total = stat.f_blocks * stat.f_frsize
            if total <= 0:
                continue
            volumes[device] = {
                'name': name,
                'used_bytes': (stat.f_blocks - stat.f_bfree) * stat.f_frsize,
                'total_bytes': total,
            }
    return volumes


def busy_percent(current, previous, milliseconds):
    if current is None or previous is None or milliseconds <= 0:
        return None, None
    # Counters can reset when devices are replaced. Treat that interval as unavailable.
    if any(new < old for new, old in zip(current, previous)):
        return None, None
    return tuple(min(100.0, 100.0 * (new - old) / milliseconds)
                 for new, old in zip(current, previous))


def main():
    smi = shutil.which('nvidia-smi')
    previous_cpu = cpu_ticks()
    previous_disks = disk_ticks()
    previous_time = time.monotonic()
    capacity = disk_capacity()
    capacity_at = previous_time
    while True:
        time.sleep(INTERVAL)
        now = time.monotonic()
        current_cpu = cpu_ticks()
        current_disks = disk_ticks()
        total_delta = current_cpu[0] - previous_cpu[0]
        idle_delta = current_cpu[1] - previous_cpu[1]
        cpu = 100.0 * (total_delta - idle_delta) / max(1, total_delta)
        if now - capacity_at >= CAPACITY_INTERVAL:
            capacity = disk_capacity()
            capacity_at = now
        disks = []
        for device, volume in capacity.items():
            read, write = busy_percent(current_disks.get(device), previous_disks.get(device),
                                       (now - previous_time) * 1000.0)
            disks.append(dict(volume, read_percent=read, write_percent=write))
        previous_cpu, previous_disks, previous_time = current_cpu, current_disks, now
        used, total = memory()
        gpu = ''
        if smi:
            try:
                result = subprocess.run(
                    [smi, '--query-gpu=utilization.gpu,memory.used,memory.total',
                     '--format=csv,noheader,nounits'], capture_output=True, text=True, timeout=5,
                    check=False)
                if result.returncode == 0:
                    gpu = next(iter(result.stdout.splitlines()), '')
            except (OSError, subprocess.TimeoutExpired):
                pass
        print(f'S {cpu:.1f} {used} {total} {gpu} | D {json.dumps(disks, separators=(",", ":"))}',
              flush=True)


if __name__ == '__main__':
    try:
        main()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
