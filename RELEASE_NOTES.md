# Chalybs v0.5.0 Release Notes

> **Status:** Released  
> **Primary Focus:** Introduction of the complete Chalybs Terminal UI (TUI)

---

## v1.0.0 – Visual Effects Baseline for chalybs-tui

### Highlights

- Introduces a dedicated **visual effects engine** for `chalybs-tui`:
  - Pulse/heartbeat glyphs for VM state.
  - Scanline-style row banding in the events panel.
  - Matrix-style drifting dot watermark with occasional glitch glyphs.
  - Subtle EMI-style border shimmer on panels (per-panel salted, deterministic).
  - Synthetic single-row load sparklines in the header and per-VM rows.
- Adds a persistent **visual effects configuration** layer:
  - `VisualEffects` struct in `tui/src/app.rs`.
  - Config file at `$XDG_CONFIG_HOME/chalybs/tui.conf` or `~/.config/chalybs/tui.conf`.
  - Simple `key = true/false` format, easily hand-edited.
- Extends the TUI shell with local-only **effects control**:
  - `effects status`, `effects on`, `effects off`.
  - `effects <pulse|scanlines|matrix|border|badges|logo|load> <on|off>`.
  - `effects save` to persist current settings.

### Upgrading from v0.5.0

- No changes are required for the daemon, PCI pipeline, or VM definitions.
- Existing users can simply rebuild `chalybs-tui` and run it; a default
  `VisualEffects` configuration (all flags = `true`) is assumed when no
  `tui.conf` exists.
- To opt out of the new visuals:
  - Run `effects off` from the TUI shell for a one-off session.
  - Or create `tui.conf` with explicit flags, e.g.:

    ```text
    pulse = false
    scanlines = false
    matrix = false
    border_noise = false
    badges = true
    logo_reactive = false
    load_index = true
    ```

### Known Limitations / Future Work

- Load sparklines currently use a deterministic synthetic pattern based on
  `tick_count` and VM index. A future release will feed these from real
  daemon-side metrics.
- `logo_reactive` is wired through configuration but behaviorally minimal in
  v1.0.0; full logo-state coupling is left for a later release.
- All effects intentionally stay within the existing color palette; no
  per-theme configuration of the effects engine is exposed yet.

---

## 1. Highlights

### 1.1 First Fully Functional TUI

This release introduces the interactive TUI for Chalybs, designed to support
both the mock backend and upcoming `chalybsd` daemon.

The interface includes:

- Three-panel layout (VMs, Events, Shell)
- Color-coded VM states
- Scroll-locking event viewer
- Shell prompt & history
- Modal overlay system for detailed VM inspection
- Brand-aligned palettes & accents
- Dedicated theme subsystem

This marks the first time Chalybs can be *operated entirely from a terminal UI*.

### 1.2 “Scrim D” Modal System

A major visual and UX enhancement:

- Opaque modal background
- Shaded border
- Scrim layer to prevent bleed-through on transparent terminals
- Centered 60×60 layout
- F2 to open, Esc to close

### 1.3 Architecture Stabilization

- Dedicated per-module responsibilities  
- Clean separation between:
  - UI state (`app.rs`)
  - Rendering (`ui.rs`)
  - Styles (`theme.rs`)
  - Logo (`logo.rs`)
- All compile-time drift resolved
- Clean event-window logic

---

## 2. New User Experience

### Panels

- **Status panel**: VM list + state glyphs  
- **Events panel**: live stream with locking  
- **Shell panel**: interaction via commands  

### Keybinds

| Key        | Action                     |
|------------|-----------------------------|
| **↑/↓**    | Select VM                  |
| **F2**     | Toggle VM detail modal     |
| **Esc**    | Close modal                |
| **Ctrl-S** | Lock events panel          |
| **Ctrl-Q** | Unlock events              |
| **PgUp**   | Scroll up                  |
| **PgDn**   | Scroll down                |
| **Enter**  | Send shell command         |
| **q**      | Quit                       |

---

## 3. Backend Integration

For now, the TUI interacts with the existing **MockBackend** which simulates:

- Tick updates  
- VM state changes  
- Events  
- Shell responses  

Full `chalybsd` integration is scheduled for v0.5.1–v0.5.3.

---

## 4. Stability & Quality

- `cargo build` — clean  
- `cargo clippy` — no functional issues  
- `cargo test` — unaffected by TUI subsystem  

Warnings exist but are strictly cosmetic (unused struct fields from mock data).

---

## 5. Looking Ahead

- Daemon integration  
- “Action palette” for VM operations  
- PCI/IRQ/NUMA heatmap visualization  
- Rich pop-out modals  
- Optional kitty-graphics rendering of the Chalybs logo  
