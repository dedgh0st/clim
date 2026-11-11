# CLIM (CLI Manager)

## Background

CLIM is an interactive terminal application for discovering and updating the package managers installed on your system.

## Supported package managers

CLIM checks for `npm`, `pnpm`, `yarn`, `bun`, `pip`, `pipx`, `poetry`, `uv`, `cargo`, `rustup`, `flutter`, `brew`, and `winget` on the current `PATH`. Detected executables are shown with their path and version when available.

## Run

```sh
cargo run
```

Use the arrow keys or `j`/`k` to navigate. Press `Space` to toggle a manager, `A` to toggle all detected managers, `Enter` (or `U`) to run the selected updates sequentially, and `Q` (or `Esc`) to quit. Detection and update output are shown in the TUI.

Update commands are manager-specific. For example, selecting Homebrew runs `brew update` followed by `brew upgrade`; selecting `winget` upgrades all applicable packages. The `cargo` selection updates the stable Rust toolchain through `rustup`.
