# benchmon // eyes on the machines

One small, always-on-top Windows panel. CPU, RAM, NVIDIA GPU, disks. Local or over SSH. Rust + egui. No central service, no telemetry.

![Benchmon monitoring five machines](assets/screenshot.jpg)

## > boot

Grab the Windows x64 ZIP from [Releases](https://github.com/Krarilotus/benchmon/releases/latest), extract it, run `benchmon.exe`. No installer or admin rights. No config? Just this PC.

Want more machines? In that same folder:

```powershell
Copy-Item hosts.example.txt hosts.txt
notepad hosts.txt
```

```text
DESKTOP|local
RIG|windows|my-workstation
SERVER|linux|my-server
```

Restart after editing. Destinations are your SSH aliases or `user@host`; usernames with spaces work. Blank lines and `#` comments are fine.

## > wire

Windows client: PowerShell 5.1 + OpenSSH on PATH. Remote Windows: PowerShell + SSH server. Remote Linux: Python 3 + SSH server + `/proc`. GPU readings use the first NVIDIA GPU through `nvidia-smi`.

Use **your own keys** and SSH config (`~/.ssh/config`). Create a key with `ssh-keygen -t ed25519` if needed; install only its **public** key on the remote account. Load passphrase-protected keys with `ssh-add`. [Windows key setup](https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh_keymanagement).

Connect manually first, verify the server fingerprint, then check:

```powershell
ssh -o BatchMode=yes -o StrictHostKeyChecking=yes my-server hostname
.\benchmon.exe --check | Out-String
```

`--check` prints one JSON reading per machine, exits nonzero on failure, and stops its probes. It uses the same configuration and collectors as the panel. No local Python needed. SSH must work without prompts; benchmon never accepts unknown host keys for you. Bring your own LAN, VPN or SSH bridge.

## > read the lights

Green CPU · red RAM · orange GPU · blue disks. Bars brighten after **50%**, rapidly after **80%**, keeping their hue. The flash is a lighter version of that hue.

Disk columns follow each volume's total capacity; each has 32 scaled segments. **I/O** = max(read busy %, write busy %), capped at 100%, **not MB/s**. **SPACE** = occupied/total. Right-hand values show the busiest disk and combined space. Hover for details. Linux `/` labels are mount points, like Windows `C:` drive letters.

Counters update roughly every 2s, capacity every 30s. One graph keeps 90 samples; missing readings leave gaps. Red link status means stale values (8s); ended probes retry after 10s. Closing the app stops monitoring.

<details>
<summary>Counter boundaries</summary>

Windows includes fixed drives with letters; Linux includes mounted local block filesystems once per device. Network/removable drives, tmpfs, Snap and overlay mounts are excluded. These are volumes, not necessarily physical disks; pooled/subvolume storage can overcount capacity. Linux used space includes reserved blocks. Missing NVIDIA readings hide the GPU row; unavailable I/O shows `n/a`.

</details>

## > hack

Windows build: Rust + Visual Studio C++ Build Tools.

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
.\tools\package.ps1    # build + portable ZIP in dist/
.\tools\install.ps1 -Destination C:\Tools\benchmon
```

Next update: `tools\install.ps1` remembers that directory, keeps your `hosts.txt`, repairs an existing taskbar pin and restarts. Pin this stable executable. For development diagnostics, copy your config beside `target/release/benchmon.exe` and run it with `--check`.

The core is six files: `main.rs` runs the window, `config.rs` validates hosts, `probe.rs` owns processes/parsing, `ui.rs` draws, and two collectors read OS counters. Icons are checked in; `tools/make-icon.py` regenerates them with Pillow. Keep the lockfile for reproducible builds.

**Keep your access yours.** Real host lists, keys, tokens and build outputs stay out of Git and release ZIPs. Packaging uses an explicit allowlist and remaps build paths. Collectors read counters, not file contents; no history is saved. Review diagnostics/screenshots before sharing.

[MIT](LICENSE). Small window. Many machines.
