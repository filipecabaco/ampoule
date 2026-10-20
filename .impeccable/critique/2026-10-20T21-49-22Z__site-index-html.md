---
target: full website review
total_score: 21
max_score: 32
na_heuristics: 7,10
p0_count: 0
p1_count: 2
target_identity: "file:/Users/filipecabaco/Documents/workspace/ampoule/site/index.html"
target_fingerprint: "sha256:9071bb72e7d8cc519a7b1a4e6f2ab9352746376b25abc73658900c572cf3909e"
target_path: /Users/filipecabaco/Documents/workspace/ampoule/site/index.html
timestamp: 2026-10-20T21-49-22Z
slug: site-index-html
---
# Critique: site/index.html (full page)

Heuristics 21/32 (7, 10 n/a). Specific above the fold; below sheet one the assembly world thins into generic docs rows.

## Priority issues
- [P1] The two steps fail as written: `chmod +x ampoule` leaves it in cwd but the next plate runs `ampoule release` from PATH; step 2 copies `my_app-x86_64-linux` though `--target` defaults to the host (macOS arm64 makes `my_app-aarch64-darwin`).
- [P1] The install plate downloads the bare binary, which lacks `kit/`; NIF apps (advertised in "Bring your NIFs") fail to pack. Use the tarball or note it.
- [P2] Every command renders at ~11.3px (`.cmd` 0.82rem × `code` 0.86em); 10.7px on phones. The CTA is the smallest text on sheet one.
- [P2] "Before you start" (OTP 28 only, one node, priv/ programs dropped) comes after the steps and all fittings.
- [P2] Measurements split/duplicated: "1.1–6×" overhead reads as a gain (word "slower" only in aside); Specifications repeats launch time (0.3 s vs 285 ms).

## Also
- ARIA: aria-labelledby on generic div.fit-group; aria-label on pre.specimen; masthead link aria-label "ampoule" vs visible "AM–0061736D"; no aria-live on copy feedback.
- JetBrains Mono not preloaded. Masthead nav wraps at 375. No version shown; no SHA256SUMS link. Step 2 has no copy button so ② never completes. Cover has ~250px dead purple at 1440. "Licence" vs "License". Mark tints #e9e2f5/#8f80ff undocumented.
