---
name: ampoule
description: The two-step assembly sheet; the visual system of the landing site in site/.
colors:
  elixir-purple: "#4e2a8e"
  wasm-blue: "#654ff0"
  lavender-wash: "#efeafa"
  sheet-white: "#ffffff"
  drawing-ink: "#17181a"
  ink-soft: "#4b4c4f"
  chart-grid: "#e4e4e6"
typography:
  display:
    fontFamily: "Familjen Grotesk, Helvetica Neue, Arial, sans-serif"
    fontSize: "clamp(4rem, 9vw, 6rem)"
    fontWeight: 700
    lineHeight: 0.85
    letterSpacing: "-0.035em"
  numeral:
    fontFamily: "Familjen Grotesk, Helvetica Neue, Arial, sans-serif"
    fontSize: "clamp(3.5rem, 5.4vw, 5.25rem)"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "-0.03em"
  headline:
    fontFamily: "Familjen Grotesk, Helvetica Neue, Arial, sans-serif"
    fontSize: "clamp(1.9rem, 4vw, 2.75rem)"
    fontWeight: 700
    lineHeight: 1.05
    letterSpacing: "-0.03em"
  title:
    fontFamily: "Familjen Grotesk, Helvetica Neue, Arial, sans-serif"
    fontSize: "clamp(1.45rem, 2.4vw, 1.9rem)"
    fontWeight: 700
    lineHeight: 1.12
    letterSpacing: "-0.02em"
  claim:
    fontFamily: "Familjen Grotesk, Helvetica Neue, Arial, sans-serif"
    fontSize: "clamp(1.35rem, 2.2vw, 1.75rem)"
    fontWeight: 600
    lineHeight: 1.2
  body:
    fontFamily: "Familjen Grotesk, Helvetica Neue, Arial, sans-serif"
    fontSize: "1.0625rem"
    fontWeight: 400
    lineHeight: 1.55
    fontFeature: "'tnum' 1"
  label:
    fontFamily: "Familjen Grotesk, Helvetica Neue, Arial, sans-serif"
    fontSize: "0.9rem"
    fontWeight: 600
    lineHeight: 1.4
  code:
    fontFamily: "JetBrains Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "0.82rem"
    fontWeight: 400
    lineHeight: 1.7
rounded:
  hairline: "3px"
  part: "4px"
  plate: "6px"
  panel: "8px"
  chip: "999px"
spacing:
  gutter: "clamp(1rem, 4vw, 3rem)"
  sheet: "clamp(3rem, 6vw, 5rem)"
  step: "clamp(1.75rem, 3vw, 2.5rem)"
components:
  button-copy:
    backgroundColor: "{colors.drawing-ink}"
    textColor: "{colors.sheet-white}"
    padding: "0 1rem"
  button-copy-hover:
    backgroundColor: "{colors.wasm-blue}"
    textColor: "{colors.sheet-white}"
  chip-target:
    backgroundColor: "{colors.sheet-white}"
    textColor: "{colors.drawing-ink}"
    rounded: "{rounded.chip}"
    padding: "0.3rem 0.75rem"
  chip-target-checked:
    backgroundColor: "{colors.wasm-blue}"
    textColor: "{colors.sheet-white}"
    rounded: "{rounded.chip}"
  command-plate:
    backgroundColor: "{colors.sheet-white}"
    textColor: "{colors.drawing-ink}"
    rounded: "{rounded.plate}"
    typography: "{typography.code}"
  the-part:
    backgroundColor: "{colors.lavender-wash}"
    textColor: "{colors.drawing-ink}"
    rounded: "{rounded.plate}"
    padding: "1.1rem 1.25rem 1.1rem 1rem"
  step-numeral:
    textColor: "{colors.drawing-ink}"
    typography: "{typography.numeral}"
    size: "clamp(5.5rem, 8vw, 8rem)"
  step-numeral-done:
    backgroundColor: "{colors.wasm-blue}"
    textColor: "{colors.sheet-white}"
---

# Design System: ampoule

## Overview

**Creative North Star: "The Two-Step Assembly Sheet."** The site is the shortest assembly manual ever printed: one part, two steps. It borrows the grammar of wordless flat-pack instructions (circled step numerals at impact scale, pictogram figures drawn in heavy uniform strokes, an exploded drawing with callouts, a spec sheet) and refuses the category default of a dark terminal hero over a feature grid.

The page is a white sheet ruled into panels by 3px ink lines. Elixir purple owns one whole panel, the cover, so a visitor reads "Elixir" before a word is parsed; WebAssembly blue marks the one file and every active state. Everything else is black ink on white.

Tone is plain and positive. The page shows what ampoule does; it never sets ampoule against other tools (no crossed-out lists of things you no longer need).

## Colors

### Primary
- **Elixir Purple** (`#4e2a8e`): the cover panel, drenched. Also the `$` prompt in commands, list markers and link underlines. It is the colour of the language this is for.
- **WebAssembly Blue** (`#654ff0`): the one file (border and ×1 tab of the part card, the release-image section of the drawing) and every active state: the checked target chip, copy hover, selection, focus halo, a finished step's numeral.

### Neutral
- **Sheet White** (`#ffffff`): the ground of every panel except the cover.
- **Drawing Ink** (`#17181a`): text, rules, pictograms, numerals, the copy buttons.
- **Ink Soft** (`#4b4c4f`): secondary text, command output, notes.
- **Lavender Wash** (`#efeafa`): the part card, inline code, the "Works" column, hover on chips.
- **Chart Grid** (`#e4e4e6`): the scaling chart's gridlines only, one step off the sheet so they recede behind the data.

### Named Rules
**The Two-Worlds Rule.** Purple is Elixir, blue is WebAssembly; the page uses exactly these two hues, at page scale, and adds no third accent. Purple fills a region; blue marks things you can act on or the file you ship.

**The Positive Rule.** Nothing is crossed out, compared or put down. Limits are stated plainly in "Before you start", next to what works.

## Typography

**Familjen Grotesk** (self-hosted, OFL) carries everything from the 6rem name to labels. **JetBrains Mono** (self-hosted, OFL) is for code, file names and the part number only.

### Hierarchy
- **Display** (700, `clamp(4rem, 9vw, 6rem)`, line-height 0.85, -0.035em): the name on the cover, once.
- **Numeral** (700, up to 5.25rem inside an 8rem circle with a 6px stroke): ① and ②. They outweigh every heading on the sheet.
- **Headline** (700, up to 2.75rem): sheet titles ("Steps 3 to 9", "Before you start", "Test report").
- **Title** (700, up to 1.9rem): step titles.
- **Claim** (600, up to 1.75rem): the one-line offer under the name.
- **Body** (400, 1.0625rem/1.55, tabular numerals): prose, 62–75ch.
- **Code** (JetBrains Mono, 0.82rem/1.7): command plates.

## Layout

- Masthead strip (mark, part number `AM–0061736D`, links) over a 3px rule.
- Sheet one is a two-column grid (`0.85fr 1.35fr`): the purple cover (name, claim, part card, facts line at its foot) beside the two steps. Each step is a grid of numeral and pictogram (left) and body (right).
- Then full-width sheets, each closed by a 3px rule: the real output specimen, Steps 3–9 (two columns), Before you start, the exploded drawing, the Test report (bench rig, scaling chart, measurements in the specification table's form), Specifications, and the back cover with `ampoule release` again.
- Side gutter `clamp(1rem, 4vw, 3rem)`; sheets pad `clamp(3rem, 6vw, 5rem)` vertically.
- Below 1020px sheet one stacks (cover on top). Below 640px each step puts the numeral and its pictogram side by side above the body, the exploded drawing switches to its vertical version, and the spec table becomes stacked rows.

## Elevation & Depth

Flat print. There are no shadows at rest. Depth is only line weight: 3px ink for structure, 1.5px hairlines for detail, dashed lines for motion and leaders. The one transient effect is the part card's `arrive` ring: a 12px wasm-blue ring that contracts to nothing once on load.

## Shapes

Square panels, cut by full-bleed rules. Radii are small and functional: 3px inline code, 4px drawing sections, 6px command plates and the part card, 8px the specimen and the rules grid, full pill only for target chips. Circles are reserved for numerals and callouts.

Pictograms are authored geometry: a filled circle head, an 18-unit body stroke, 10-unit limbs, 4-unit furniture lines, all round-capped. They are wordless and consistent; never shaded or perspectived.

## Components

### Step numeral
A circle with a 6px ink stroke and a 700-weight digit, upright and optically centred: no letter-spacing on the single digit, and `padding-bottom: 0.13em` puts its ink within 1px of the circle's centre at every size (measured on renders; the OG card uses 12.5px at its 84px digits). Copying a step's command fills it wasm blue with a white digit; it never tilts. The small variant (3.25rem, 3px stroke) numbers Steps 3–9; the back cover repeats ① at 4.75rem.

### Scaling chart
The Test report's one chart, drawn like the sheet: a 1.5px ink axis, 1px hairline gridlines, the ideal as a dashed ink line, the measured series as the page's single blue line (2.5 units, round joins) with 5-unit blue markers ringed in white, and one direct label on the endpoint. A dotted divider marks where the performance cores end. Each marker carries a `<title>`; the table beneath carries every value. Below 640px the labels grow in chart units so they print at about 12px.

### Command plate
A 1.5px ink outline on white, mono text, `$` prompt in Elixir purple, with the copy button docked on the right edge (ink fill, white label, copy icon). Hover turns the button wasm blue. On success the label reads "Copied" and the icon becomes a check for 1.8 s; if the clipboard is refused the text is selected and the label reads "Selected".

### Target chips
Pills with a 1.5px ink outline. The checked chip is wasm blue with white text; hover is lavender wash. The "Download for" label sits on its own line above them.

### The part card
The single item in the inventory: lavender wash, 3px wasm-blue border, the mark at 4rem, the file name in mono, a one-sentence description, and a solid blue ×1 tab.

### Exploded drawing
An inline SVG of the file: five outlined sections (4-unit ink stroke) pulled apart along a dashed axis, hairline detail inside, the release image filled wasm blue, the optional engine section dashed, dashed leaders down to circled callouts that match the numbered legend below. A vertical version replaces it on phones.

### Do / don't columns
"Works" (lavender wash) and "Does not work" (white) inside one 3px-ruled panel, each headed by a drawn circled ✓ or ✗ symbol.

### Specifications table
Label left, measured value right at 1.35rem/700, 1.5px rules between rows, 3px rule on top.

## Do's and Don'ts

### Do:
- **Do** use Elixir purple as a field (the cover) and WebAssembly blue for the file and active states, nothing else.
- **Do** keep step numerals as the largest marks after the name.
- **Do** draw any new figure or diagram in the pictogram grammar: uniform round-capped strokes, ink on white, one blue accent at most.
- **Do** regenerate the mark with `scripts/logo.exs` and paste its colours into the `#mark` symbol in `site/index.html`.
- **Do** keep every number and claim to what PRODUCT.md lists as evidence.

### Don't:
- **Don't** cross out, compare or disparage other tools.
- **Don't** add a third accent colour, gradients, glows or card shadows.
- **Don't** put a small label above a heading.
- **Don't** use a dark terminal hero or a grid of icon cards.
- **Don't** shade or put perspective on pictograms.
