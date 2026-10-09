# benchmon

A small, always-on-top Windows window for monitoring this PC and your Windows or Linux machines over SSH. Written in Rust with egui. No remote benchmon installation, central server, cloud account, or telemetry service is required.

## What it shows

- CPU load and RAM used/total.
- NVIDIA GPU load and VRAM used/total, when `nvidia-smi` is available. The first GPU is shown; other GPU vendors are currently unsupported.
- Local disks arranged side by side. Each disk's width is proportional to its total capacity.
- Two stacked segmented rows within each disk: **I/O** shows `max(read busy %, write busy %)`; **SPACE** shows occupied capacity as a percentage of that disk's own capacity. Each disk has its own 32 segments scaled to its width. The same scan animation used by CPU/RAM/GPU runs slowly across all filled disk segments, at one segment per second, and repeats immediately after the final filled segment. Its highlight is a lighter shade of the current bar color.
- Values beside the disk bars show the busiest disk's I/O percentage and overall occupied percentage with used/total capacity. The disk strip aligns with the other metric bars.
- A single small history graph: CPU green, RAM dark red, GPU orange, disk I/O light blue. Bars and labels use these same colors, so no separate legend is needed. Disk history is the busiest disk's I/O percentage. Missing readings leave gaps rather than inventing zero activity.
- Online/link status, automatic reconnection, and scrolling when the machine list exceeds the window height.

Hover over a disk for its name, read/write activity, occupied/total GiB, and free GiB. Small partitions retain their actual proportional width, so their details may be easiest to read on hover.

CPU/RAM/GPU/I/O are sampled approximately every two seconds, plus the time spent collecting counters. Disk capacity is refreshed every 30 seconds. The graph retains 90 samples, approximately three minutes. Bars and values change to warning shades at 70% and critical shades at 90%; graph lines and labels keep their identifying colors. CPU progresses green/amber/red, RAM dark red/red/pink, GPU orange/gold/hot orange, and DISK light blue/strong blue/violet.

**Disk I/O measures time busy, not a percentage of the drive's advertised MB/s.** Concurrent I/O can produce counters above 100%; display values are capped at 100%. Windows uses logical-volume performance counters; Linux uses per-device read/write milliseconds from `/proc/diskstats`. These are practical activity indicators, not a disk throughput benchmark.

## Run it

1. Download the Windows x64 ZIP from [Releases](https://github.com/Krarilotus/benchmon/releases).
2. Extract it and run `benchmon.exe`. With no configuration file, only this Windows PC is monitored.
3. To add machines, copy `hosts.example.txt` to **`hosts.txt` beside the executable**, edit it with your own destinations, and restart benchmon.

The portable executable does not need an installer or administrator privileges. Windows PowerShell 5.1 must be available as `powershell`. Remote monitoring also requires the OpenSSH client, available as `ssh` on PATH. Remote Windows PCs need PowerShell and an SSH server; remote Linux PCs need an SSH server, Python 3, and `/proc`.

Example configuration:

```text
# Display name | platform | SSH destination
This PC|local
Workstation|windows|my-windows-pc
Server|linux|my-linux-server
```

Destinations can be SSH aliases or `username@hostname`. Windows usernames containing spaces are supported. Blank lines and full-line comments beginning with `#` are ignored. The supported platforms are `local`, `windows`, and `linux`; `local` means the Windows PC running the app. Invalid configuration is shown in the window instead of silently skipping machines.

An SSH alias keeps ports, usernames and key paths in your own SSH configuration:

```sshconfig
# Your own ~/.ssh/config (on Windows: %USERPROFILE%\.ssh\config)
Host my-linux-server
    HostName server.example.com
    User your-user
    Port 22
    IdentityFile ~/.ssh/id_ed25519
```

Network connectivity is your responsibility: use your LAN, VPN, or an SSH bridge you manage. Benchmon does not expose a listening port or create public tunnels.

## Set up your own SSH access

Benchmon uses your installed OpenSSH client and its existing configuration. It does not manage passwords, provision keys, or include anyone else's access.

1. Create your own key with `ssh-keygen -t ed25519` if you do not already have one. Keep the private key on your PC. Install **only the public `.pub` key** in the remote account's authorized keys.
2. For a passphrase-protected key, make it available through your SSH agent, for example with `ssh-add`. Set up the agent using your operating system's instructions.
3. Connect manually to each configured destination first: `ssh my-linux-server`. Verify the server's host-key fingerprint through a trusted channel before accepting it. Benchmon requires an existing trusted host key and will not accept a new one automatically.
4. Confirm noninteractive access works: `ssh -o BatchMode=yes -o StrictHostKeyChecking=yes my-linux-server hostname`. Benchmon cannot prompt for a password or key passphrase.

For Linux servers, the public key normally goes in `~/.ssh/authorized_keys` (directory mode 700, file mode 600). On Windows, the authorized-keys location and permissions depend on whether the account is an administrator; follow [Microsoft's OpenSSH key-management instructions](https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh_keymanagement).

## Disk coverage and limitations

Windows lists mounted fixed logical drives with drive letters (`C:`, `D:`, etc.). Removable drives, network shares, and volumes without drive letters are excluded. Linux lists mounted local block filesystems once per device, with mount-point labels (`/`, `/home`, etc.); bind mounts are deduplicated. Temporary filesystems, network mounts, Snap squashfs mounts, and container overlay filesystems are excluded.

Disk means a **mounted volume/filesystem**, not necessarily a separate physical drive. Multiple partitions of one physical disk can appear separately. Windows logical-volume and Linux device counters can differ in accounting. Filesystems with shared storage pools or subvolumes (for example ZFS/Btrfs) are not guaranteed to produce a physical-capacity total; verify their readings independently. Linux occupied capacity is total blocks minus free blocks, including blocks reserved by the filesystem.

Unavailable disk counters show `n/a`; capacity can still be shown. GPU rows are omitted when NVIDIA readings are unavailable. A disconnected machine retains its last readings with a red link status: those values are stale. Readings become stale after eight seconds without a sample, and ended probes reconnect after ten seconds. The two-second cadence is approximate; slow counters or SSH links increase it.

## Troubleshooting

- **CONFIG ERROR:** check the syntax of `hosts.txt` beside the executable and restart.
- **CONNECTING / LINK LOST:** run the noninteractive SSH test above; check connectivity, your SSH alias, host-key trust, key/agent access, and the server's SSH service.
- **No disk I/O on Windows:** check that `Get-CimInstance Win32_PerfFormattedData_PerfDisk_LogicalDisk` returns counters for the drive. Benchmon does not change Windows performance-counter settings.
- **No Linux sample:** check `ssh my-linux-server python3 --version` and permissions for `/proc`, mount information, and filesystem statistics.
- **No GPU row:** check `nvidia-smi` on that machine. Only the first NVIDIA GPU is currently used.

For source-level diagnostics, run `python tools/check-probes.py` from the repository on Windows. It reads your ignored `hosts.txt` in the repository root, checks one sample per configured machine, prints metric summaries, and stops its temporary probes. This helper requires Python 3 locally; the normal Windows app does not.

## Build and maintain

Install the Rust toolchain and Visual Studio C++ Build Tools on Windows, then:

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

The executable is `target/release/benchmon.exe`. Copy your own `hosts.txt` beside it if needed. Or use `powershell -NoProfile -File tools/package.ps1` to build a portable ZIP containing only the executable, generic example configuration, README and license. Packaging remaps local build paths and uses an explicit file allowlist.

For repeat updates, use `powershell -NoProfile -File tools/install.ps1 -Destination C:\Tools\benchmon` once, then run `tools/install.ps1` without a destination. It remembers that directory in an ignored `install-location.local` file, replaces only that directory's executable, preserves `hosts.txt`, repairs the icon on an existing pin targeting it, and restarts the app. With no saved destination it uses the repository root. Pin that stable executable using Windows **Pin to taskbar**; avoid pinning anything under `target/` or launching several copies.

The code is intentionally small:

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Application state, sampling history, and window lifecycle |
| `src/config.rs` | Host-file validation and safe local-only default |
| `src/probe.rs` | Process/SSH lifecycle, wire-format parsing, and metric types |
| `src/collectors/windows.ps1` | Windows CIM and optional NVIDIA readings |
| `src/collectors/linux.py` | Linux procfs/filesystem and optional NVIDIA readings |
| `src/ui.rs` | Bars, proportional disk layout, animation and combined graph |
| `tools/check-probes.py` | One-sample collector diagnostics |
| `tools/package.ps1` | Build and allowlisted portable packaging |
| `tools/install.ps1` | Repeat updates at one remembered local deployment path |
| `build.rs`, `assets/` | Embedded Windows executable/window icons |

The icons are checked in. Regeneration is optional: install Pillow locally and run `python tools/make-icon.py`; Pillow is not required to build or run benchmon.

Collectors stream a line beginning `S`, followed by CPU percentage, RAM used/total in KiB, optional NVIDIA CSV values (utilization and VRAM in MiB), and ` | D ` followed by a JSON array of per-disk readings. Numeric storage capacities use bytes. Old metric lines without the disk suffix remain parseable. Unknown disk activity stays unavailable. Add tests when changing configuration, the protocol, or capacity/percentage calculations.

The Windows build owns its local probe/SSH processes through a kill-on-close job. Ended probes are reaped before reconnecting. Remote collectors stream over SSH without writing files; they exit when their connection closes. Closing benchmon ends monitoring.

## Privacy

This repository and release packages contain **generic examples only**. No personal host list, SSH keys, account credentials, or access tokens are required or distributed. Your `hosts.txt`, keys, local logs, build outputs and archives are ignored by Git; do not override those exclusions when publishing. Keep your real SSH configuration in your user profile.

The app sends its collector script to each destination you configure and receives metrics over SSH. It reads system counters and filesystem statistics, not file contents. It makes no analytics or cloud requests and stores no metric history on disk. Diagnostic output and screenshots can reveal your machine names or disk layout; review them before sharing.

MIT licensed. See [LICENSE](LICENSE).
