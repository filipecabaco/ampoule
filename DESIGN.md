---
name: ampoule
description: The package insert folded inside ampoule's carton; the visual system of the landing site in site/.
colors:
  carton-amber: "#7a3a0b"
  break-ring-gold: "#f2c14e"
  beam-light: "#ffe7a8"
  carton-ink: "#fbf1e1"
  carton-ink-soft: "#f1cfa4"
  leaflet-paper: "#f6f5f1"
  typeset-ink: "#17181a"
  ink-soft: "#55524b"
  crease-rule: "#c9c4b8"
  warning-red: "#a3261b"
  terminal-ground: "#1d1b17"
  terminal-ink: "#ece7dc"
  terminal-dim: "#a8a093"
typography:
  display:
    fontFamily: "Archivo, Helvetica Neue, Arial, sans-serif"
    fontSize: "clamp(4.5rem, 12.5vw, 6rem)"
    fontWeight: 880
    lineHeight: 0.82
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 78"
  headline:
    fontFamily: "Archivo, Helvetica Neue, Arial, sans-serif"
    fontSize: "1.6rem"
    fontWeight: 650
    lineHeight: 1.12
    fontVariation: "'wdth' 85"
  title:
    fontFamily: "Archivo, Helvetica Neue, Arial, sans-serif"
    fontSize: "1.5rem"
    fontWeight: 750
    lineHeight: 1.25
  subtitle:
    fontFamily: "Archivo, Helvetica Neue, Arial, sans-serif"
    fontSize: "1.0625rem"
    fontWeight: 700
    lineHeight: 1.55
  body:
    fontFamily: "Archivo, Helvetica Neue, Arial, sans-serif"
    fontSize: "1.0625rem"
    fontWeight: 400
    lineHeight: 1.55
    fontFeature: "'tnum' 1"
  label:
    fontFamily: "Archivo, Helvetica Neue, Arial, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 600
    lineHeight: 1.55
  code:
    fontFamily: "Martian Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "0.8rem"
    fontWeight: 400
    lineHeight: 1.7
    fontVariation: "'wdth' 87.5"
  stamp:
    fontFamily: "Martian Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "0.72rem"
    fontWeight: 600
    letterSpacing: "0.05em"
    fontVariation: "'wdth' 87.5"
rounded:
  stamp: "3px"
  control: "6px"
  terminal: "8px"
  pill: "999px"
spacing:
  gutter: "clamp(1.25rem, 2.5vw, 2.25rem)"
  section-top: "2.75rem"
  subsection-top: "1.75rem"
components:
  button-copy:
    backgroundColor: "{colors.typeset-ink}"
    textColor: "{colors.break-ring-gold}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "0.55rem 0.9rem"
  button-copy-on-amber:
    backgroundColor: "{colors.beam-light}"
    textColor: "{colors.typeset-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "0.55rem 0.9rem"
  chip-target:
    backgroundColor: "{colors.break-ring-gold}"
    textColor: "{colors.typeset-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.pill}"
    padding: "0.3rem 0.65rem"
  chip-target-selected:
    backgroundColor: "{colors.typeset-ink}"
    textColor: "{colors.break-ring-gold}"
    rounded: "{rounded.pill}"
  band-install:
    backgroundColor: "{colors.break-ring-gold}"
    textColor: "{colors.typeset-ink}"
    padding: "1rem {spacing.gutter} 1.1rem"
  carton:
    backgroundColor: "{colors.carton-amber}"
    textColor: "{colors.carton-ink}"
    padding: "{spacing.gutter}"
  panel-leaflet:
    backgroundColor: "{colors.leaflet-paper}"
    textColor: "{colors.typeset-ink}"
    padding: "0 {spacing.gutter} calc({spacing.gutter} * 1.5)"
  terminal:
    backgroundColor: "{colors.terminal-ground}"
    textColor: "{colors.terminal-ink}"
    typography: "{typography.code}"
    rounded: "{rounded.terminal}"
    padding: "1rem 1.15rem"
  command-plate:
    backgroundColor: "{colors.break-ring-gold}"
    textColor: "{colors.typeset-ink}"
    rounded: "{rounded.control}"
    padding: "0.75rem 1.1rem"
---

# Design System: ampoule

Scope: the GitHub Pages site in `site/` (`index.html`, `style.css`, `main.js`, `favicon.svg`, `fonts/`). The CSS custom properties in `site/style.css` (`--paper`, `--ink`, `--amber`, `--gold`, ...) are the source of these tokens; the frontmatter names them by role.

## Overview

**Creative North Star: "The Package Insert"**

The site is the leaflet folded inside ampoule's carton. The visitor opens the box: on the left an amber carton face printed in one ink, carrying the name at carton scale and a gold break-ring band that holds the install command; on the right, and then below in a three-panel unfolded sheet, white leaflet paper typeset in near-black, in numbered sections that follow a medicine leaflet's order (what it is, before you use it, how to use it, possible side effects, how to run and store it, contents of the pack). Limits are printed as warnings, at the same weight as capabilities.

Density is that of printed matter: long measures capped at 58–68ch, hairline rules between items, numbers set tabular, real terminal output instead of illustration. The material is paper and printed card, not glass and glow: depth comes only from the carton lying over the leaflet and from the creases between leaflet panels. One beam line, gold fading to nothing, crosses from the band over the seam toward section 1.

The site deliberately refuses the dark terminal hero plus feature-card grid that CLI tools ship. Terminals appear only as specimens inside the leaflet, never as the page's ground.

**Key Characteristics:**
- Two materials: amber carton (`carton-amber`) and leaflet paper (`leaflet-paper`), never mixed within a panel.
- One gold band per carton face, carrying the one primary action (install, or the closing `ampoule release`).
- Numbered leaflet sections with medicine-leaflet headings; warnings as plain lists with red markers.
- Lot-and-batch stamp block in uppercase mono on the carton.
- Panels meet at fold creases and unfold on scroll; copying snaps the break ring.
- No purple anywhere on the site.

## Colors

A warm two-material palette: dark amber glass-and-card, warm white paper, near-black ink, and gold reserved for the break ring and action.

### Primary
- **Carton Amber** (`carton-amber`): the carton face (the left 45% of the first viewport) and the bottom flap (footer). Also the link colour on paper, the `theme-color`, the scrollbar thumb, `accent-color` and caret. 7.90:1 as link text on paper.
- **Break-Ring Gold** (`break-ring-gold`): the install band, the closing command plate, the `$` prompt in terminals, the text of ink-filled buttons and selected chips, `::selection`, the skip link. Gold is a surface for action or a mark of the ring, never body text on paper.

### Secondary
- **Beam Light** (`beam-light`): everything that is "light" on amber: links on the carton and flap, the one-ink mark stamp, the focus outline on amber, the copy button on the flap. 7.08:1 on amber.

### Neutral
- **Leaflet Paper** (`leaflet-paper`): the page ground and every leaflet panel.
- **Typeset Ink** (`typeset-ink`): body text and headings on paper (16.29:1), text on gold (10.59:1), the ring line, ink-filled buttons and selected chips, the default focus outline.
- **Ink Soft** (`ink-soft`): intros, captions, table asides, the revision line (7.14:1 on paper).
- **Crease Rule** (`crease-rule`): 1px hairlines between table rows, fact rows, the contents block and the sheet's top edge.
- **Carton Ink** (`carton-ink`): text printed on amber (7.70:1).
- **Carton Ink Soft** (`carton-ink-soft`): secondary text and stamp labels on amber (5.83:1).
- **Warning Red** (`warning-red`): only the list markers of "Do not use ampoule if your app" (6.75:1 on paper). The warning text itself stays ink.
- **Terminal Ground / Terminal Ink / Terminal Dim** (`terminal-ground`, `terminal-ink`, `terminal-dim`): the code specimens. Commands in terminal ink (13.94:1), program output in terminal dim (6.64:1), prompts in gold (10.25:1).

### Named Rules

**The No-Purple Rule.** No violet, purple or lilac appears anywhere on the site: not in CSS, not in the favicon, not in an inline SVG, not in a gradient, not as a hover state. This is a binding user decision. The mark is the four bytes every WebAssembly module starts with, `00 61 73 6D`, as a 2×2 grid of tiles with seven-segment hex digits (`assets/logo.svg`, generated by `scripts/logo.exs`): `00` gold `#f2c14e` with ink digits, `61` and `73` amber `#7a3a0b` with beam `#ffe7a8` digits, `6D` ink `#17181a` with gold digits and a gold rim. On the site it appears in two cuts only: the one-ink stamp (`#mark` symbol in `site/index.html`, tiles printed in `currentColor`, digits knocked out through a mask) and the full-colour favicon (`site/favicon.svg`, the same file as `assets/logo.svg`). Any new mark use takes one of these two cuts.

**The One-Ink Carton Rule.** Everything printed on amber is in one ink family: carton ink for type, beam light for links, the mark and focus. No second accent appears on the carton except the gold band.

**The Gold Is the Ring Rule.** Gold surfaces carry the action (install band, command plate) or mark the ring (ink-on-gold chips and buttons, `$` prompts). It is never used for decoration or for body text on paper.

**The Measured Contrast Rule.** Every text pair is AA or better, and the ratios above were measured. A new pair is measured before it ships.

## Typography

**Display Font:** Archivo (variable, weight 100–900, width 62–125%), self-hosted from `site/fonts/archivo-latin.woff2`, with Helvetica Neue, Arial, sans-serif.
**Label/Mono Font:** Martian Mono (variable, weight 300–600, width 75–112.5%), self-hosted from `site/fonts/martian-mono-latin.woff2`, with ui-monospace, SF Mono, Menlo, monospace. Both are under the OFL; the licence files sit beside the fonts.

**Character:** One grotesque does the whole leaflet, and its width axis does the work a second display face would: condensed (78%) and very heavy for the name on the carton, slightly condensed for claims and headings, normal width for reading. Martian Mono, set at 87.5% width, is the typewriter of the stamps, terminals and measured figures.

### Hierarchy
- **Display** (880, width 78%, `clamp(4.5rem, 12.5vw, 6rem)`, line-height 0.82, -0.03em): the name "ampoule" on the carton face, once per page. The flap's closing line is a sibling at width 70%, weight 820, `clamp(2rem, 5vw, 3.5rem)`.
- **Headline** (650, width 85%, 1.6rem, 1.12): the carton claim, max 22ch, balanced. The leaflet title uses the same size at 750, width 90%.
- **Title** (750, 1.5rem, 1.25): numbered leaflet section headings ("1. What ampoule is..."), balanced.
- **Subtitle** (700, 1.0625rem): subsection headings inside a section ("Take special care", "Measured").
- **Body** (400, 1.0625rem, 1.55, tabular numerals): leaflet text, capped at 68ch (58ch for the intro), `text-wrap: pretty`.
- **Label** (600–650, 0.875rem): chips, buttons, carton links, footer links, captions.
- **Code** (Martian Mono, width 87.5%, 0.8rem in terminals and the install command, 0.86em inline, 1.7 line-height in terminals).
- **Stamp** (Martian Mono, 600 values / regular labels, 0.72rem, uppercase, 0.05em): only the lot-and-batch block on the carton.

### Named Rules

**The Width-Axis Rule.** Hierarchy is made with Archivo's weight and width axes, not with a second display family. Narrower and heavier means louder.

**The Uppercase-Is-a-Stamp Rule.** Uppercase letterspaced mono belongs to the carton's lot stamp, a printed-carton device. It is not used as a section label or kicker above headings in the leaflet.

## Layout

The first viewport is the opened box: a two-column grid at 45fr / 55fr, at least `min(100svh, 58rem)` tall so very tall screens do not stretch the carton. Carton on the left, leaflet section 1 on the right with a slightly deeper left gutter (1.4× the gutter).

Below it, the leaflet unfolds as a sheet: three equal panels on wide screens, with the last panel (section 6, contents of the pack) spanning two columns as the wide back page; two columns at or below 80rem; one column at or below 56rem, where the carton stacks above the leaflet and the beam line is hidden. At or below 30rem the install command and copy button stack, and the file-layout bar turns vertical.

One fluid gutter (`spacing.gutter`) drives all panel padding; panels add 1.5× at the bottom. Sections open with 2.75rem above the heading, subsections 1.75rem. Lists inside sections space items 0.4rem apart. Content inside the leaflet never runs wider than 68ch even when the panel is wider. The page ends on the amber bottom flap, which repeats the one action and the source links.

## Elevation & Depth

The page is flat print. Depth exists only where paper physically overlaps or folds, and every shadow is a soft, ink-tinted, negative-spread shadow.

### Shadow Vocabulary
- **Carton over leaflet** (`box-shadow: 18px 0 40px -18px rgb(23 24 26 / 0.45)`; on narrow screens `0 18px 40px -18px` downward): the carton lying on top of the paper.
- **Fold crease** (`border-left: 1px solid rgb(255 255 255 / 0.9)` plus `box-shadow: -1px 0 0 rgb(23 24 26 / 0.16), inset 2rem 0 1.75rem -1.75rem rgb(23 24 26 / 0.13)`; rotated to the top edge on one column): every panel after the first in a row meets the previous one at a lit edge, a dark seam and a valley shade.
- **Terminal specimen** (`box-shadow: 0 10px 30px -14px rgb(23 24 26 / 0.5)`): a code block sitting slightly off the page.

### Named Rules

**The Paper-Only Depth Rule.** Shadows come from paper geometry (overlap, crease) or a specimen laid on the page. No glows, no hover lifts, no card shadows on content.

## Shapes

Print-sheet geometry: carton, band, panels and flap are square-edged, full-bleed rectangles. Radii are small and functional: 3px for the stamp block's ruled frame, 6px for buttons, the command plate and the file-layout bar, 8px for terminals, full pill only for the target chips. The band bleeds to the carton's edges with negative inline margin. The mark is a tilted vial (rotated 20°), always rendered from the `#mark` symbol.

## Components

### Buttons
- **Shape:** gently rounded (6px), icon plus word, 0.55rem × 0.9rem.
- **Copy (on gold):** ink fill, gold text, 650 weight label, copy icon (inline `#icon-copy` symbol). On success the label reads "Copied" and the icon swaps to `#icon-check` for 1.8 s; if the clipboard is refused, the command text is selected and the label reads "Selected".
- **Copy on amber:** beam-light fill, ink text, used on the flap.
- **Active:** `translateY(1px)` over 0.15s on `cubic-bezier(0.16, 1, 0.3, 1)`. No hover colour change.
- **Focus:** 2px solid outline, offset 3px, ink on paper and gold, beam light on amber.

### Chips
- **Style:** the four target selectors are visually hidden radio inputs inside labels; each chip is a pill with a 1px border at `rgb(23 24 26 / 0.55)` on gold, ink text.
- **State:** hover darkens the border to full ink; checked fills ink with gold text; keyboard focus draws a 2px ink outline offset 2px.

### Cards / Containers
- **Carton:** amber field, one-ink type, gutter padding, a 1.25rem flex gap between its parts.
- **Leaflet panel:** paper ground, gutter padding, folded to its neighbour by the crease shadow. Panels are not cards: no radius, no border box, no hover.
- **Terminal:** dark ground, 8px radius, specimen shadow, horizontal scroll at full width; inside narrow panels it wraps (`pre-wrap`, `overflow-wrap: anywhere`) so a command's end stays visible.

### Navigation
The only navigation is the leaflet's own contents list ("What is in this leaflet"): an ordered list in two balanced columns between hairline rules, plain amber underlined links (1px underline, 0.2em offset, 2px on hover). A skip link, gold with ink text, sits off-screen until focused. Footer links sit on the flap in beam light beside a small mark stamp.

### Install Band (signature)
The break ring: a full-bleed gold band on the carton holding "Download for" target chips, the install command in Martian Mono, and the copy button. The command wraps only at spaces and at `<wbr>` after each `/` of the base URL, and the asset name never breaks. A gold beam line leaves the band's right edge across the seam (hidden on one column). Copying plays the ring snap: a 3px ink line draws across the band's bottom edge and fades (0.9s, ease-out).

### Lot Stamp
A 1px frame at 70% carton ink, 3px radius, two-column grid of `dt`/`dd` pairs in uppercase Martian Mono: Lot, Contents, For, Licence, and a full-width Targets row.

### Measured Table and Facts List
Rows separated by crease-rule hairlines. The measured table puts the claim left in Archivo and the figure right in bold Martian Mono, with an optional soft-ink aside under the figure. The facts list is an 11rem term column beside the description, collapsing to stacked on one column.

### File-Layout Bar
A proportional strip of the file's sections in order, 1px ink frame, 6px radius, ink dividers: runtime in amber, release image in gold, engine in a paper diagonal hatch, manifest plain paper, trailer solid ink. Labels sit at the bottom of each segment.

### Leaflet Unfold (motion)
Where `animation-timeline: view()` is supported and motion is not reduced, each panel unfolds from `perspective(1600px) rotateX(-14deg)` and 0.94 brightness to flat over the first 70% of its entry. With reduced motion, panels are static and smooth scrolling is off.

## Do's and Don'ts

### Do:
- **Do** print the mark from the `#mark` symbol in one ink (`currentColor`), and use `site/favicon.svg` (the full-colour mark) where a coloured mark is needed.
- **Do** keep amber surfaces one-ink: carton ink for text, beam light for links, marks and focus.
- **Do** put the page's single primary action on gold, with ink text, and nothing else on gold surfaces.
- **Do** set every terminal from real command output: commands in terminal ink, output in terminal dim, prompts in gold and unselectable.
- **Do** state limits as a leaflet warning list with red markers and ink text, beside the capabilities.
- **Do** build hierarchy with Archivo's weight and width axes; keep body text at 1.0625rem with tabular numerals and a 68ch cap.
- **Do** measure the contrast of every new text pair and keep it at AA or better.
- **Do** honour `prefers-reduced-motion`: no unfold, no smooth scroll.

### Don't:
- **Don't** use purple, violet or lilac anywhere on the site, and don't draw or hand-edit the mark: regenerate it with `scripts/logo.exs`.
- **Don't** turn the page into a dark terminal hero with a feature-card grid; terminals are specimens on paper.
- **Don't** add glows, hover lifts or drop shadows to content; depth is carton overlap, fold creases and terminal specimens only.
- **Don't** use gold as text on paper or as decoration.
- **Don't** put uppercase letterspaced labels above leaflet headings; uppercase mono is the carton's lot stamp only.
- **Don't** add testimonials, logos, download counts or unmeasured figures to the measured table.
