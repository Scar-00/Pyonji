# Further visual polish

Work continues on `ui-improvement-review` after the committed round-22 build.
Review scores are subjective assessments, not test results. Reviewers are not
told the desired score. Preserve graphite/lilac styling, keyboard-first tabs,
UI-font prose, mono code/paths, wgpu rendering, and transient scrollbars.

## Recovered iterations

Rounds 23–25 introduced shared overlay headers/fields/footer sizing, palette
grouping and shortcut hints, summary matching, contextual opener actions,
release typography changes, completion descriptions, tab pane counts, and
pixel-aligned split dividers. These changes remain uncommitted.

The historical Opus scores were 8.6 for round 23 and 8.5 for round 24.
Its older round-22 reappraisal was 7.6; it does not grade the later changes.

## Round 25 — Sol 6.1, medium

All 18 actual screenshots inspected. Weighted score: **8.1**.

| Area | Weight | Score |
|---|---:|---:|
| Terminal / splits | 20% | 7.8 |
| Tabs / status | 10% | 7.8 |
| Lua completion / signature / prompt | 20% | 8.5 |
| Palette | 15% | 8.5 |
| Sessions | 10% | 8.3 |
| Opener | 10% | 7.8 |
| Releases | 15% | 8.0 |

Findings: inactive foreground faded 50%; opener secondary button did not name
its current-directory target; release footer repeated the target triple;
palette instructional prose used mono. Optional suggestions: improve tab
identity and separate session metadata.

## Changes for round 26

- Reduce inactive-pane foreground fade to 18%, preserving colors for reference.
- Name the opener's secondary action with its directory; Enter still browses
  the selected directory or opens the selected file.
- Remove the release footer's repeated target triple. Keep the replacement
  binary path, keyboard guidance and install action.
- Use UI font for palette instructions and mono for argument/command tokens.
- Add separators between session ID, tab and current-session state.
- Preserve shell-provided tab titles: identical sessions do not have distinct
  directory or process context to invent. Numeric positions and pane counts
  remain available; custom names can be set with rename.

Validation: locked build, all 74 tests, formatting, Clippy with warnings denied,
and `git diff --check` pass. Fresh screenshots are captured under
`/home/ahri/.cache/pyonji-ui-review/round26` before the next visual review.

## Round 26 — Sol 6.1, medium

All 18 screenshots inspected independently: **8.59 weighted, rounded to 8.6**.
Terminal 8.7, tabs 8.5, Lua 8.5, palette 8.7, sessions 8.8, opener 8.5,
releases 8.4. The reviewer confirmed that all five round-26 refinements worked
visibly and that identical shell tab titles were appropriate to the context.

Remaining findings: opener's selected action was implicit; signature counters
were hard to parse; long edge fades resembled erased rows; release prose had a
long reading measure and a raw changelog URL.

## Changes for round 27

- Opener footer explicitly says Browse/Open beside the selected target path.
- Signature header consolidates Arguments plus arg count into Argument N of M;
  overload navigation is separated on the right and uses N of M wording.
- Shared overflow fades shorten from 20px to 8px, preserving transient thumbs.
- Release notes cap prose width at 560px.
- Changelog extraction handles GitHub Markdown links as well as plain URLs;
  the external link uses a descriptive label without repeating the raw URL.
  A regression test checks both forms and preservation of ordinary prose.

Screenshots: `/home/ahri/.cache/pyonji-ui-review/round27`.

The first round-27 scrolled capture exposed another real changelog form:
`**Full changelog:** URL` (colon inside the bold marker). Extraction and the
regression test now cover that form too. The three release screenshots are
recaptured against the corrected build; the other 15 remain current.
Locked build, all 75 tests, formatting, Clippy with warnings denied, and diff
whitespace checks pass.

## Round 27 — Sol 6.1, medium

All 18 screenshots inspected: **8.93 weighted overall**. Terminal 9.0, tabs
8.9, Lua 9.1, palette 8.8, sessions 9.1, opener 8.6, releases 8.9.
No major visible usability defects. The requested 9.5 has not been reached.

Remaining minor notes: expose keyboard activation for the opener's secondary
current-directory action; lift supporting metadata; mask the clipped palette
row beneath its pinned label. Generous release-empty-state proportions were
classified as taste only. Follow-up investigation began, but no changes for
these findings were implemented before the user requested commit and push.

Rounds 23–27 are included in the committed work. The screenshot artifacts stay
in the local cache; this report records the implementation and validation.
