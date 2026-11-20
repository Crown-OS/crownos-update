# crownos-update

crownos-update moves an installed CrownOS system from one release to the next
release. It does one release at a time. Before each update it makes a snapper
snapshot and keeps the current kernel, so you can go back from the boot menu.

## Crates

| Crate | Kind | Purpose |
|---|---|---|
| `update-manifest` | library | Release versions, the signed `manifest.toml`, the release chain and the channel rules. The `fetch` feature adds the download and the signature check against the pacman keyring. |
| `update-proto` | library | The `crownos-ipc` contract: `state`, `check`, `download`, `apply`, `restart`, the `StateChanged` and `Progress` events, and the `Snapshot` types. |
| `update-client` | library | `UpdateClient` for apps such as Settings: connect, call the verbs and read the events in order. |
| `crownos-updated` | binary | The root daemon. It runs each step and keeps the state. |
| `crownos-update-notify` | binary | A user service. It shows one notification for each new release. A click opens Settings on System Updates. |

## The socket

The daemon listens on `/run/crownos/crownos-updated.sock` with mode 0666.
Every local user can read the state and receive the events. Only root and the
members of `wheel` can call `check`, `download`, `apply` and `restart`. Other
users get the error code `UNAUTHORIZED`. A verb that does not fit the current
state gets `WRONG_PHASE` (100), with a sentence that tells why.

## The update steps

1. **Check**: a stable install reads the `stable` manifest and takes the
   first release in its `releases` list that is after the installed release.
   Then it reads the manifest and the notes of that release from its
   `v<X.Y.Z>` channel. A testing install stays on `testing`: it takes the
   version of the `testing` manifest when that version is after the installed
   one. Tags are not candidates for a testing install. All files must have a
   valid CrownOS signature.
2. **Download**: `pacman -Syuw` with a copy of `pacman.conf` that points
   `[crownos]` at the next release. A check starts the download
   automatically.
3. **Apply**: make a snapper snapshot, keep the kernel pair, write the
   rollback entry for rEFInd, run one `pacman -Syu`, point `/etc/pacman.conf`
   at the new release and run `/usr/lib/crownos/post-update` if the release
   has one. When an apply fails after the snapshot, or the daemon stops during
   it, the next apply uses the same snapshot and continues after the last
   completed step. It does not make a second snapshot.
4. **Restart**: `systemctl reboot`, `systemctl soft-reboot` or a session
   restart, as the release asks.

The daemon checks 15 minutes after it starts and then every 24 hours. When a
scheduled check fails, for example because the computer is offline, the
daemon only writes the error to the log. When a user starts the check, the
daemon shows the failure.

On SIGTERM the daemon stops the socket, lets the running verb finish and then
exits with status 0. A running apply goes to its end: the upgrade, the channel
switch, the post-update script and the applied marker. The `TimeoutStopSec` of
the unit limits the wait. Verbs that are still in the queue do not run.

### The post-update script

- The script runs as root after the pacman upgrade, with
  `CROWNOS_FROM_VERSION` and `CROWNOS_TO_VERSION` set.
- Make the script safe to run again. If the daemon stops during the
  script, or the script fails, the next apply runs it again.
- Every local user can read the output of the script, because the log tail
  is part of the state. Do not write secrets to the output.

## The state files

The daemon keeps its state in `/var/lib/crownos/update/`:

| File | Content |
|---|---|
| `state.toml` | The last check time, the next release and the last failure. |
| `next-manifest.toml` | The signed manifest of the next release, as downloaded. |
| `next-notes.md` | The release notes of the next release. |
| `in-progress.toml` | Written before the pacman upgrade and after each later step: the two releases, the rollback snapshot and the last completed step. The daemon removes it when the update is complete. While it is there, only `apply` is allowed. |
| `applied.toml` | Written after a successful update. It stays until the restart. |

An example `state.toml` after a failed post-update script:

```toml
last_check = 1795158726

[next]
version = "0.4.1"
restart = "soft-reboot"
new_packages = ["crowndock"]
download = "complete"

[failure]
message = "post-update failed with exit status: 1"

[failure.step]
apply = "post-update"

[failure.rollback]
snapshot = 7
from = "0.4.0"
```

An example `in-progress.toml` after the pacman upgrade:

```toml
from = "0.4.0"
to = "0.4.1"
restart = "soft-reboot"
new_packages = ["crowndock"]
completed = "install"

[rollback]
snapshot = 7
from = "0.4.0"
```

An example `applied.toml`:

```toml
version = "0.4.1"
from = "0.4.0"
restart = "soft-reboot"
boot_id = "boot-1"
soft_reboots = 0
```

## Run the daemon against a fixture

You do not need root. Make a directory that has the files the daemon reads,
and a directory of fake tools:

- `ROOT/etc/os-release` with `VERSION_ID=0.4.0`.
- `ROOT/etc/pacman.conf` with a `[crownos]` section and
  `Server = https://repo.crownos.org/v0.4.0/$arch`.
- `ROOT/etc/passwd` and `ROOT/etc/group` that put your user in `wheel`.
- `ROOT/etc/kernel/cmdline`, `ROOT/efi/EFI/refind/` and
  `ROOT/usr/share/pacman/keyrings/crownos-trusted`.
- `TOOLS/pacman`, `TOOLS/snapper`, `TOOLS/gpg`, `TOOLS/systemctl` and
  `TOOLS/crownos-esp-kernel`.
- A repository with `stable/manifest.toml`, `v0.4.1/manifest.toml`, the
  notes and a `.sig` file for each.

Then start the daemon:

```sh
CROWNOS_UPDATE_ROOT=$ROOT \
CROWNOS_UPDATE_TOOLS=$TOOLS \
CROWNOS_REPOSITORY_URL=file://$REPOSITORY \
RUST_LOG=debug \
cargo run -p crownos-updated
```

The socket is then `$ROOT/run/crownos/crownos-updated.sock`.
`UpdateClient::connect()` uses the same `CROWNOS_UPDATE_ROOT`, so start
Settings with it to use the fixture daemon. `crownos-updated/tests/support/`
builds a complete fixture; read it for an example.

## Test

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

MIT or Apache-2.0.
