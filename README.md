# clim

**A terminal UI for finding and updating package managers.**

`clim` scans your `PATH` for supported package managers, shows their installed
versions, and lets you check for updates and run selected upgrades one at a
time.

## Features

- Detects supported package managers and displays their executable paths and
  versions.
- Checks for newer versions where an update check is available.
- Runs selected updates sequentially and streams command output in the TUI.
- Keeps npm update checks compatible with the active Node.js version.
- Reports update failures and verification results in the interface.
- Prints a read-only detection report when run without an interactive terminal.

## Supported package managers

| Manager | Update behavior |
| --- | --- |
| `npm` | Updates npm using the detected installation method (NVM, Homebrew, Volta, or the active Node installation). System-owned `/usr/bin` installations are not overwritten. |
| `pnpm` | Runs `pnpm self-update`, with a Corepack fallback when applicable. |
| `yarn` | Installs the latest Yarn globally through npm. |
| `bun` | Runs `bun upgrade`. |
| `pip` | Upgrades pip. |
| `pipx` | Installs or upgrades pipx with pip. |
| `poetry` | Runs `poetry self update`. |
| `uv` | Runs `uv self update`. |
| `cargo`, `rustup` | Updates the stable Rust toolchain through rustup. |
| `flutter` | Runs `flutter upgrade`. |
| `brew` | Runs `brew update` and `brew upgrade`. |
| `winget` | Runs `winget upgrade --all`, which upgrades applicable installed applications, not WinGet itself. |

Detection is based on executables available on the current `PATH`. Update
checks are available for most managers; WinGet does not have a check for
updating WinGet itself. Checks use public registries or release endpoints and
require network access.

### npm and Node.js compatibility

The latest npm release may require a newer Node.js version than the one
currently active. `clim` checks npm against the active Node.js version and
selects the newest compatible npm release, rather than blindly targeting the
registry's absolute latest. After an npm update, it verifies the version
reported by the detected npm executable.

## Getting started

### Requirements

- Rust and Cargo (stable toolchain recommended).
- A supported package manager installed and available on `PATH`.
- A terminal that supports raw input and an alternate screen for interactive
  use.

### Install a prebuilt release

Prebuilt archives are published for Linux x86_64 and ARM64, macOS Intel
(x86_64) and Apple Silicon (aarch64), and Windows x64. Visit
[GitHub Releases](https://github.com/dedgh0st/clim/releases/latest) to see all
downloads and release notes.

#### macOS

Choose the archive for your Mac:

```sh
# Apple Silicon
curl -fL https://github.com/dedgh0st/clim/releases/latest/download/clim-aarch64-apple-darwin.tar.gz -o clim.tar.gz

# Intel
curl -fL https://github.com/dedgh0st/clim/releases/latest/download/clim-x86_64-apple-darwin.tar.gz -o clim.tar.gz

tar -xzf clim.tar.gz
sudo install -m 755 clim /usr/local/bin/clim
```

#### Linux

```sh
# x86_64
curl -fL https://github.com/dedgh0st/clim/releases/latest/download/clim-x86_64-unknown-linux-gnu.tar.gz -o clim.tar.gz

# ARM64
curl -fL https://github.com/dedgh0st/clim/releases/latest/download/clim-aarch64-unknown-linux-gnu.tar.gz -o clim.tar.gz

tar -xzf clim.tar.gz
sudo install -m 755 clim /usr/local/bin/clim
```

#### Windows

Run the following in PowerShell to install the latest x64 release in your
user-local programs directory and add it to your user `PATH`:

```powershell
$release = "https://github.com/dedgh0st/clim/releases/latest/download/clim-x86_64-pc-windows-msvc.zip"
$archive = Join-Path $env:TEMP "clim.zip"
$installDir = Join-Path $env:LOCALAPPDATA "Programs\clim"

Invoke-WebRequest -Uri $release -OutFile $archive
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Expand-Archive -Path $archive -DestinationPath $installDir -Force

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ";") -notcontains $installDir) {
    $newPath = (@($userPath, $installDir) | Where-Object { $_ }) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
}
```

Open a new terminal, then launch the app with:

```powershell
clim.exe
```

To update, rerun the installation steps to download and replace the latest
release.

### Run from source

```sh
git clone https://github.com/dedgh0st/clim.git
cd clim
cargo run
```

To build an optimized executable:

```sh
cargo build --release
```

The binary is written to `target/release/clim`.

## Using the TUI

1. Wait for package manager detection to finish.
2. Move through the list and select the managers you want to update.
3. Optionally check for available updates.
4. Press Enter to run selected updates sequentially.

| Key | Action |
| --- | --- |
| `↑` / `↓`, `k` / `j` | Navigate the list |
| `Space` | Toggle the highlighted manager (or select/deselect all on the first row) |
| `A` | Toggle selection of all detected managers |
| `C` | Check detected managers for updates |
| `Enter` or `U` | Update selected managers sequentially |
| `Q`, `q`, or `Esc` | Quit |

Managers known to be up to date from the latest check are skipped when you
start an update. Update checks and upgrades may take time; their status and
command output are shown in the TUI.

> **Before updating:** `brew upgrade` can upgrade all outdated Homebrew
> packages, and `winget upgrade --all` can upgrade all applicable installed
> Windows applications. Review the selected managers and their update scope
> before pressing Enter.

## Development

Run the test suite and standard Rust checks before submitting changes:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Contributions, bug reports, and suggestions are welcome. Please include
reproduction steps for bugs and describe the relevant operating system and
package manager versions.

## CI and releases

GitHub Actions runs tests on Linux x86_64 and ARM64, macOS, and Windows for
pushes to `main` and pull requests. Formatting and Clippy checks run on Linux.

To publish a release, update the version in `Cargo.toml` and `Cargo.lock`, then
run `cargo check` to refresh the lockfile, then commit the version change and
push a matching version tag. For example:

```sh
cargo check
git tag v0.1.1
git push origin v0.1.1
```

The release workflow builds platform archives, creates a GitHub Release with
generated notes, and marks it as the latest release. The installation
instructions above always download assets from that latest release.
