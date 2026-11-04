# Five further UI reviews (rounds 18–22)

Branch: `ui-improvement-review`, rebased onto main `99a54be`.
Reviewers: Sol 6.1 and Opus 5.5, both at medium effort. Every round uses fresh
screenshots, read-only reviews, and the same seven-component weighted rubric.
Scores are assessments, not automated test results.

## Direction and user preferences

Preserve the graphite/lilac terminal UI and keyboard-first tabs. System UI font
for navigation; configured mono font for code and paths. Reuse shared components.
The user specifically rejected permanent scrollbars and scoped rendering to
wgpu. Use transient hover/scroll thumbs and native overflow fades.

## Fork audit and fix

- Pulled local `/home/ahri/dev/oss/gpui-ce` and
  `/home/ahri/dev/oss/gpui-component` from their `fork/main` remotes.
- Core update `c165ef597e`: ordered filters, richer gradients, alpha masks,
  blend modes, rounded-border antialiasing, and wgpu startup improvements.
- Components update `a4277291`: custom dialog/notification shadows survive
  animation.
- Existing GPUI tooltip/Windows event edits were preserved and remain uncommitted.
- Native masks exposed missing text: the subpixel pipeline wrote RGB without
  alpha, making text disappear when an isolated group was composited.
- Fixed shader coverage alpha and enabled alpha writes. Added pixel regression
  tests for masks/opacity over a colored backdrop, including uncovered texels.
- Also joined pending pipeline compiler threads during renderer destruction:
  detached workers caused Vulkan validation-layer crashes at process shutdown.
- Published the isolated fix as
  [b7b61ab0a1](https://github.com/Scar-00/gpui-ce-fork/commit/b7b61ab0a1b016218bb9d7d211bdc46b43b48c46)
  on `fix/masked-text-alpha`; Pyonji pins that revision.
- GPUI validation: `cargo test -p gpui_ce_render -p gpui_ce_wgpu --features gpui_ce_wgpu/test-support --lib --tests`
  passes all 112 tests and exits cleanly. Before the fix, the new masked-text
  pixel test failed and broader tests crashed at exit.

## Scores

| Round | Sol | Opus |
|---|---:|---:|
| 18 | 9.1 | 8.8 |
| 19 | 9.1 | 8.9 |
| 20 | 9.1 | 9.0 |
| 21 | 9.1 | 9.0 |
| 22 | 9.1 | 9.1 |

## Iterations

- Before round 18: updated fork dependencies; replaced palette `uniform_list`
  with variable-height `list` (26px headings, 38px commands). Persistent
  `ListState`, measured scrollbar, and keyboard selection through section rows.
- After round 18: removed height slack in palette and sessions; made the clear
  release filter a standard button. Initially added persistent overflow bars.
- After round 19: increased notes scrollbar clearance, hid inert release hints,
  made inactive palette input gutter transparent, and added shared edge fades.
- Before round 20 review: honored user preference by changing all explicit
  permanent bars to hover/scroll mode. Native-mask experiment exposed the core
  bug; graded screenshots used working paint-time gradient fades.
- After round 20: fixed sessions sizing to use the visible filtered count;
  omit search when there are no sessions and omit meaningless footer text.
  Fixed the GPUI mask/lifecycle issues and restored native content masking.
- Round 21: both verified native masking with no disappearing text. Opus found
  an initial-height regression in detached-empty sessions. Direct refresh during
  draw was being ignored; palette and sessions now defer the refresh until after
  draw. Removed redundant Nothing selected text from the release footer.
- Consolidated focused divider data into renderer Chrome; Clippy with warnings
  denied now passes. All 76 app tests and formatting checks pass.
- Round 22: both reviewers give 9.1 overall, with every component at least 9.0.
  The compact detached dialog and cleared release footer are verified. No
  significant visible usability defect remains in the final screenshots.
- Remaining optional notes: deeper completion overflow fade; a possible single
  initial frame at the prior dialog size (source-only, not observed). Measuring
  inside an app update is deliberately avoided to prevent reentrant reads.

## Final validation and repository state

- Pyonji: 76 tests pass with --locked; cargo fmt --check passes;
  cargo clippy --locked -- -D warnings passes; git diff --check passes.
- GPUI: 112 renderer/shader tests pass and exit cleanly, including the new pixel
  regression and dropping a renderer with pending compiler workers.
- Pyonji changes remain uncommitted on ui-improvement-review.
- GPUI fix is committed and pushed on fix/masked-text-alpha and pinned by Pyonji.
  No PR was opened. The user's three unrelated local GPUI edits remain untouched.
- All five joint rounds (18–22) are complete; no additional rounds were started.

Round 22 reviews: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3A452ea620-e926-42a7-9407-3f1b98ad36c2>

<t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3Aa0933def-b100-4115-ac56-266d8b4d8777>

Round 21 reviews: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3A3f8bfc36-c961-4010-b099-27c855c4043b>

<t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3A3dda2471-dc29-4000-ba96-02895dd796e5>

## Evidence

Screenshots and full reviewer findings:
`/home/ahri/.cache/pyonji-ui-review/round18` through `round22`.
Each contains PNGs and `sol-review.md` / `opus-review.md`.
The expanded capture includes keyboard navigation to the last palette command,
session no-match/detached-empty, selected-folder, release no-match/scrolled states.
Native-mask diagnostic images (not graded):
`/tmp/pyonji-mask-native/mask.png` and `/tmp/pyonji-mask-smoke/mask.png`.

### Round 18

Sol: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3Ab7c0851f-5589-4387-b403-b674847ffcb4>

Opus: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3A01500050-d808-44a7-89f2-f740d11a8070>

### Round 19

Sol: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3Aa2a13298-02f0-435d-9796-ba197f2e26b0>

Opus: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3Aa4e9098c-7e59-4847-80f8-16f5b432a937>

### Round 20

Sol: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3Aa22e6d28-45be-4162-b5d2-addd4e6c5c37>

Opus: <t3-thread://v2/c6d3234a-f079-42a2-9f10-df87e0a83fbe/thread:delegated-task:command%3Amcp%3A0dd4e2ee-9239-4a7e-a593-21c8b2d9e328%3Adelegate-task%3Afe51c063-0b54-4410-a13c-f008020314a9>
