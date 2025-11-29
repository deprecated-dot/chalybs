
# Halo Rendering Bug Reference
> **Chalybs TUI – PNG Logo Halo Issue (Set C)**
>
> Status as of v1.1.0: **Known Issue / Experimental Feature**

This document captures the context, symptoms, and constraints around the
**Chalybs logo halo** rendering problem in the TUI, so that future work can
fix it **once** rather than repeating debugging loops.

---

## 1. Context

The TUI has a brand-aligned **PNG logo** rendered (where supported) using
`viuer`, plus an experimental **dual-wing halo** behind the logo.

Key components:

- `tui/src/logo.rs`
  - Owns the logical logo slot in the status panel.
  - Splits a fixed-height region into:
    - Top: PNG area (delegated to `logo_png::draw_png_logo`).
    - Bottom: text caption (“CHALYBS ⟐”).

- `tui/src/logo_png.rs`
  - Detects terminal image backend (Kitty / iTerm / none).
  - Resolves PNG path and height scaling.
  - Emits the PNG via `viuer::print_from_file`.
  - Implements the **Set C halo** (C3 / C3Narrow / C3Wide / C3ExtraWide).

- `tui/src/app.rs`
  - Owns `VisualEffects` and `logo_halo: LogoHaloProfile`.
  - Exposes `effects halo <off|c3|c3narrow|c3wide|c3extrawide>` via the
    TUI shell.

The intent: **a subtle C-shaped dual-wing halo hugging the logo**, breathing in
sync with the rune, rendered *behind* the PNG so the transparent logo can sit
cleanly on top.

---

## 2. What Went Wrong (High-Level)

The bug is **not** “the halo is drawn under the ASCII fallback”. The user
confirmed repeatedly—and with screenshots—that:

- The halo *is* drawn in the PNG path.
- The halo changes shape correctly when switching profiles (`effects halo c3`,
  `c3narrow`, `c3wide`, etc.).
- The halo responds to the breathing curve.

But:

1. The halo was anchored and scaled against the **wrong geometry**.
2. Vertical placement caused the bottom of the wings to be **cropped**.
3. Horizontal centering was computed against the **full left panel**, not the
   PNG logo slot.
4. Several attempted “fixes” repeatedly misinterpreted the screenshots and
   blamed the ASCII logo, even when the ASCII path wasn’t in play.

The result: 24+ hours burned on circular attempts that never fully addressed
the geometry mismatch.

---

## 3. Observed Symptoms

Based on the user’s screenshots and descriptions:

1. **Cropped Bottom Edge**
   - For all halo profiles, the bottom of the wings was cleanly *cut off*,
     even when the profile shapes were correct.
   - This indicates the halo was being drawn into a rect too short, or
     vertically centered in a region that didn’t match the intended
     7-row mask height.

2. **Misaligned Vertical Placement**
   - The halo band appeared in the **center** of the top-left status panel,
     not wrapped tightly around the PNG.
   - In some iterations, the halo clearly sat *behind* where the PNG should be,
     but offset vertically so that the logo intruded only into its top half.

3. **Incorrect Horizontal Reference**
   - At least one implementation computed centering against the **entire
     left column**, ignoring the smaller PNG slot defined by `logo::draw_logo`.
   - This produced visually “floating” wings, offset from the actual logo.

4. **Profile Logic Correct, Geometry Wrong**
   - Changing the profile (`c3`, `c3narrow`, `c3wide`, `c3extrawide`) clearly
     changed the shape and width of the halo.
   - The logical mask math was mostly fine; the **placement** wasn’t.

5. **Aesthetic Mismatch**
   - Even in the closer iterations, the halo read as “Cyberpunk club neon”
     rather than “Gibsonian console glow”.
   - It did not match the project’s ethos (“forged in Linux…”) or the subtle
     aesthetic of the rest of the TUI.

---

## 4. Root Cause Categories

The exact combination of mistakes varied across iterations, but they fell into
three categories.

### 4.1 Wrong Rect (PNG vs Panel)

The TUI layout code does this (simplified):

- `ui.rs::draw_status_panel` gives a **7-row slot** to `logo::draw_logo`.
- `logo::draw_logo` splits that:
  - `png_area` (top N rows).
  - `caption_area` (bottom 2 rows) for the breathing text.

The halo should be aligned with `png_area`, i.e. the region where the PNG is
drawn. However, several halo attempts instead:

- Centered inside the **entire left panel** (VM status region).
- Or ignored the PNG/caption split, effectively centering across all 7 rows.

### 4.2 Incorrect Vertical Mapping

The halo uses 7×7 masks (`MASK_ROWS = 7`, `MASK_COLS = 7`) and then maps them
into the target `halo_rect` with:

- `mask_row = (row * MASK_ROWS) / halo_height`.

Some iterations combined this with:

- A `halo_height` that was smaller than the PNG slot.
- A halo rect that was vertically **centered** instead of top‑anchored.

Result: bottom rows of the mask were mapped outside the visible region, so
the most intense “foot” of the wings was simply not drawn.

### 4.3 Wrong Horizontal Anchoring

The halos were also horizontally centered using `area.width` from a region that
did not correspond to the actual PNG width.

The user confirmed that:

- The halo wings were visibly behind the logo, but shifted
  and never wrapped around the rune the way the design intended.

---

## 5. Constraints & Expectations (Future Work)

Future halo work must satisfy the following **hard constraints**.

### 5.1 Geometry Constraints

1. **Rect of Reference**
   - The halo must be drawn relative to the **exact PNG slot**:
     - Same origin `(x, y)` as the PNG.
     - Same height as the PNG area (`png_height` in `logo::draw_logo`).

2. **Height**
   - Logical halo height is `MASK_ROWS` (7 rows).
   - If `png_height < 7`, the halo must scale gracefully but **never crop** the
     bottom of the wings.
   - If `png_height > 7`, halo can either:
     - Stay 7 rows high at the top of the PNG, or
     - Be vertically centered *within the PNG area*, never outside it.

3. **Width**
   - Halo width must be a narrow band hugging the logo.
   - It should not extend to the full width of the left panel unless the logo
     does.
   - The wings should form a dual-sided “bow tie” with a **clear center gap**.

4. **Alignment**
   - The halo must feel visually symmetric around the rune/PNG centerline.
   - No cropped bottoms, no visible horizontal offset from the logo.

### 5.2 Aesthetic Constraints

The target aesthetic is:

- Subtle, Gibson-ish console glow.
- Breathing gently with `logo_breath_factor`.
- Within the existing ACCENT_TEAL/ACCENT_PINK/ACCENT_PURPLE palette.
- Never dominating the status panel or looking like “night-club neon puke”.

If a proposed halo design fails that test, it should be treated as **rejected**.

### 5.3 Behavioral Constraints

- The halo is **purely cosmetic**.
- It must never affect daemon behavior, VM state, PCI pipeline, or config.
- It must be toggleable via:
  - `effects halo off|c3|c3narrow|c3wide|c3extrawide`.
- Defaults for stable releases should be conservative:
  - `halo = off` by default until the behavior is correct and aligned with
    the brand.

---

## 6. Current Status (v1.1.0)

As of this checkpoint:

- The halo pipeline is present but considered **experimental**.
- Geometry and aesthetics are **not yet acceptable**.
- The feature is a known issue and should be treated as such in:
  - `CHANGELOG.md`
  - `RELEASE_NOTES.md`
  - `ROADMAP.md` (as a future TUI refinement).

Best practice for now:

- Keep halo **off by default** for real work.
- Use the experimental halo only when actively iterating on the effect.

---

## 7. Recommended Path Forward

When revisiting the halo in a future version (e.g. v1.2.x):

1. **Lock the Geometry First**
   - Explicitly log or print the rectangles you believe you’re using.
   - Confirm that the halo rect == PNG rect for origin + height.
   - Add a debug profile that draws the halo rect border in a single color.

2. **Validate With Fixtures**
   - Capture reference screenshots for:
     - `halo = off`
     - `halo = c3`
     - `halo = c3narrow`
     - `halo = c3wide`
     - `halo = c3extrawide`
   - Use those as visual fixtures for future changes.

3. **Only Then Tune Aesthetics**
   - Once the geometry is correct and stable, adjust intensity, glyph choices,
     and color blending to match the desired Gibson-ish feel.

4. **Update Docs & Bump Version**
   - When the halo is finally correct, update:
     - `CHANGELOG` (documenting the fix and new default if applicable).
     - `RELEASE_NOTES` (user-facing explanation).
     - `CHALYBS_EXECUTION_AND_ARCHITECTURE` (TUI subsection).

This document is the capsule of “what went wrong” so that you never have to
reconstruct the 24-hour loop from memory again.
