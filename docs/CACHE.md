# Moving Build Caches to a Separate Disk

Rust and C++ build caches grow large quickly — a busy Rust project can accumulate
200GB+ in `~/.cargo/target` within weeks. If your home directory is on a small SSD
and you have a large data disk mounted elsewhere, moving these caches is straightforward.

---

## What Needs Moving

| Cache | Default Location | Typical Size | Purpose |
|-------|-----------------|--------------|---------|
| Cargo registry + binaries | `~/.cargo` | 2–5 GB | Downloaded crates, installed binaries |
| Cargo build artifacts | `~/.cargo/target` or per-project `target/` | **50–300 GB** | Compiled objects, incremental cache |
| ccache | `~/.cache/ccache` | 5–20 GB | C/C++ compiler output cache |
| sccache | `~/.cache/sccache` | 5–20 GB | Rust + C/C++ shared compiler cache |

The **build artifact directory** is by far the largest. This is what fills disks.

---

## Step 1: Move the Existing Directories

Replace `$BIG_DISK` with your data disk mount point (e.g. `/data`, `/mnt/storage`, `/projects`).

```bash
BIG_DISK=/data   # change this to your mount point

# Create destinations
mkdir -p $BIG_DISK/cargo $BIG_DISK/cargo-target $BIG_DISK/ccache $BIG_DISK/sccache

# Move Cargo home (registry, credentials, installed binaries)
# rsync preserves permissions and removes source files as it goes
rsync -a --remove-source-files ~/.cargo/ $BIG_DISK/cargo/
find ~/.cargo -type d -empty -delete

# Move build artifacts (may take a while if large)
rsync -a --remove-source-files ~/.cargo-target/ $BIG_DISK/cargo-target/
find ~/.cargo-target -type d -empty -delete

# Move ccache if you use it
if [ -d ~/.cache/ccache ]; then
    rsync -a --remove-source-files ~/.cache/ccache/ $BIG_DISK/ccache/
fi
```

> **Note:** Use `rsync --remove-source-files` rather than `mv` for large directories.
> It moves incrementally, is resumable, and avoids filesystem rename restrictions
> across mount points.

---

## Step 2: Create Backward-Compatibility Symlinks

Many tools hardcode `~/.cargo`. Symlinks keep everything working without any
other changes to project configs:

```bash
BIG_DISK=/data   # same as above

ln -sfn $BIG_DISK/cargo ~/.cargo
ln -sfn $BIG_DISK/cargo-target ~/.cargo-target
```

---

## Step 3: Set Environment Variables

Use `/etc/environment` as the **single source of truth**. It is read by PAM on
login and applies to all sessions: interactive shells, graphical apps (VSCode,
GNOME), and system services alike.

Edit `/etc/environment` directly (use `sudo`):

```
PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/snap/bin:$BIG_DISK/cargo/bin"
CARGO_HOME=$BIG_DISK/cargo
CARGO_TARGET_DIR=$BIG_DISK/cargo-target
CCACHE_DIR=$BIG_DISK/ccache
CCACHE_MAXSIZE=20G
SCCACHE_DIR=$BIG_DISK/sccache
```

> **Syntax rules:** `/etc/environment` uses plain `KEY=VALUE` — no `export`, no
> shell expansion, no variable references between entries. Append the cargo bin
> path directly into the `PATH` line rather than referencing `$CARGO_HOME`.

> **Do not** also set these variables in `/etc/profile.d/` or `~/.bashrc`. Doing
> so creates duplicate, potentially conflicting definitions that override
> `/etc/environment` silently. One location — `/etc/environment` — is enough.

---

## Step 4: Verify

Open a new terminal (or re-login for GUI apps) and check:

```bash
cargo --version
echo $CARGO_HOME        # should show $BIG_DISK/cargo
echo $CARGO_TARGET_DIR  # should show $BIG_DISK/cargo-target

# Build something and confirm artifacts land on the big disk
df -h $BIG_DISK
```

---

## Per-Project Target Directories

`CARGO_TARGET_DIR` sets a **global** target directory used by every project.
This means all projects share one location, which saves space but means you
can't run two project builds simultaneously on different Cargo versions without
conflicts.

**Alternative: per-project override** — add to each project's `.cargo/config.toml`:

```toml
[build]
target-dir = "/data/cargo-target/my-project"
```

Or set per-session:

```bash
CARGO_TARGET_DIR=/data/cargo-target/my-project cargo build
```

---

## C++ / CMake

CMake doesn't have a universal cache variable like Cargo does. The standard
approach is to use an out-of-source build directory on the big disk:

```bash
# Instead of: cmake -B build
cmake -B /data/cmake-builds/my-project

# Or set a default in your shell profile:
export CMAKE_DEFAULT_BUILD_DIR=/data/cmake-builds
```

If using **ccache**, setting `CCACHE_DIR` (done above) is sufficient. Enable it
in CMake with:

```cmake
find_program(CCACHE ccache)
if(CCACHE)
    set(CMAKE_C_COMPILER_LAUNCHER ${CCACHE})
    set(CMAKE_CXX_COMPILER_LAUNCHER ${CCACHE})
endif()
```

---

## Disk Space Maintenance

Even on a big disk, build caches grow indefinitely. Clean periodically:

```bash
# Remove all Cargo build artifacts (safe — rebuilt on next cargo build)
cargo clean --target-dir $CARGO_TARGET_DIR

# Or remove only a specific project's artifacts
cargo clean

# Show what's using space
du -sh $CARGO_TARGET_DIR/* 2>/dev/null | sort -h | tail -20

# Trim old Cargo registry cache (keeps downloaded crates but removes old versions)
cargo cache --autoclean   # requires: cargo install cargo-cache
```

---

## Troubleshooting

**`~/.cargo/env: No such file or directory`** — The Rust installer adds `. "$HOME/.cargo/env"`
to `~/.bashrc`. After moving `CARGO_HOME`, update that line:

```bash
# Old (broken after move)
. "$HOME/.cargo/env"

# New
[ -f "$CARGO_HOME/env" ] && . "$CARGO_HOME/env"
```

**Cargo credentials not found** — Credentials live in `$CARGO_HOME/credentials.toml`.
After moving `CARGO_HOME`, confirm the file exists there:

```bash
ls $CARGO_HOME/credentials.toml
```

**VSCode doesn't pick up the new paths** — `/etc/environment` changes require
re-login (full graphical session restart) to take effect for GUI apps. For the
current session, set the vars manually or restart VSCode from a terminal that
has the updated environment.

**Two Cargo versions fighting over the same target dir** — Use per-project
`.cargo/config.toml` with different `target-dir` paths if you need strict isolation.
