# CLIM (CLI Manager)

## Background

CLIM is an interactive terminal application for discovering and updating the package managers installed on your system.

## Supported package managers

CLIM checks for `npm`, `pnpm`, `yarn`, `bun`, `pip`, `pipx`, `poetry`, `uv`, `cargo`, `rustup`, `flutter`, `brew`, and `winget` on the current `PATH`. Detected executables are shown with their path and version when available.

## Run

```sh
cargo run
```

Use the arrow keys or `j`/`k` to navigate. Press `Space` to toggle a manager, `A` to toggle all detected managers, `C` to check detected managers for newer versions, `Enter` (or `U`) to run the selected updates sequentially, and `Q` (or `Esc`) to quit. Update checks use the relevant public package registry or release endpoint and require network access. Detection, check failures, and update output are shown in the TUI.

Update commands are manager-specific. For example, selecting Homebrew runs `brew update` followed by `brew upgrade`; selecting `winget` upgrades all applicable packages. For npm, clim detects common NVM, Homebrew, and Volta installations and uses their respective update commands; other npm installations update npm in the active Node installation. Its update check considers the npm release's Node.js engine requirements, so it only offers the newest release compatible with the active Node.js version. After updating, clim verifies the npm version it actually runs. OS-owned npm installations are not overwritten directly. The `cargo` selection updates the stable Rust toolchain through `rustup`.
