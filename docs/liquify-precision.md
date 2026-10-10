# Experimental precision Liquify

This fork carries the cumulative fourth-pass Liquify package on
`liquify/precision-pass4-validation`. The package was applied after synchronizing
`main` with `storytold/photocraft` at
`7722172585a01cbdb93c06f0f5ff2634fcb17999` on 2026-10-10. All four host source
blobs still matched the package's inspected baseline; the installer validated
100 replacements and 15 files before writing any changes.

The original ZIP, notes, and extracted package remain local and are excluded
through `.git/info/exclude`. They are not application dependencies.

Open **Filter > Liquify**, then turn on **Enable face adjustments**. Detection is
**off by default**. Google MediaPipe finds the face landmarks automatically; no
eye or mouth boxes are required. The photo is processed on this computer. Google's
published native runtime sends usage/performance metrics; the opt-in UI links to
[its privacy notice](https://github.com/google-ai-edge/mediapipe#privacy-notice).

The first version adjusts one detected face. Eyes can be linked or adjusted
separately: size, height, width, tilt, spacing, and closure. Mouth controls cover
smile, width, height, and the upper/lower outlines. Nose controls cover width and
length. Face shape covers width, forehead, jawline, and chin length. Zero means
the detected starting shape. Sliders rebuild one model-independent face operation
from that starting shape, so repeated previews do not accumulate deformation.

**Edited photo / Original photo** compares the result. **Reset face** restores
the current face adjustment; **Apply** commits the session and **Cancel** discards
it. Turning face controls off keeps the existing edit and restores access to
brush tools. A brush edit invalidates old detection, so turning face controls on
again detects the changed photo. Face sliders are one undoable operation within
the session; Apply is one document history step. Reopened Smart Filters retain
saved slider values and replay their contours without needing the model.

The view has **Fit**, **100%**, an editable zoom percentage, and **-/+**. Ctrl+wheel
and touchpad pinch now zoom about the pointer, as ordinary wheel scrolling does.
Select the Hand tool (**H**) to pan, or hold Space / use the middle mouse button.
Select Zoom (**Z**) to click-zoom; Alt-click zooms out. Fit remains active on resize.
100% now uses physical image pixels even on a scaled Windows display.

Brush settings and the original manual eye/mouth guide prototype are available
under **Brush tools and advanced options**. This remains a geometric warp, not
texture reconstruction: extreme edits can stretch skin and cannot invent teeth,
iris detail, or hidden facial areas. Use a reasonably large, front-facing,
well-lit face. Whole-face edits use a four-million-node interaction cap (the same bound as the
session); manual guide edits keep their 500,000-node cap.
An oversized or unstable deformation reports an error and keeps the last result.
Native detail previews are bounded to a 4 MP visible region. Reopened Smart Filters
preview the isolated Liquify operation rather than the complete filter stack.

Webcam/reference-photo expression transfer is a later stage, per the requested
priority of feature sliders first. Detection is currently native-only; saved
face deformations still replay in the Rust core without MediaPipe.

## Build and validation

Use Rust 1.95+ and the prerequisites in [development.md](development.md).

The personal Windows executable is `dist/windows/photocraft.exe`. Launch it
directly; Cargo is only needed when changing the source. Rebuild it with:

```powershell
./scripts/build-windows-personal.ps1
```

The script builds the native Windows app with two jobs, line-table debug symbols,
and incremental compilation disabled, then copies the executable and any runtime
DLLs into `dist/windows`, and installs the pinned MediaPipe model and Windows
runtime. It does not build installers or release packages.
After copying the app, the generated `target` directory can be removed when
keeping the source tree compact matters more than faster incremental rebuilds.
The initial full debug/test cache reached 81.5 GB and was removed. Keeping the
finished Windows app does not require retaining the generated build cache.

The additional Liquify GitHub workflow is manual and Windows-only. Pushing this
feature branch does not automatically run it.

For broader developer validation, use a new output directory:

```powershell
python -X utf8 tools/liquify-validation/runner.py . --output log/liquify-validation/new-run
```

Choose a new output directory for each validation run. The runner retains
compiler, test, lint, WASM, and build results in `summary.json` and gate logs.
A native build alone does not establish that every check passed or that portrait
editing behaves correctly. Interactive portrait QA and performance comparisons
remain separate work.

### Automatic face controls: validation

On 2026-10-10 the native checks passed: **29 algorithm tests, 58 Liquify UI
tests, 15 engine distortion tests, one model-input test, and the hostile-input
command test**. The real-detector portrait regression passed separately, including
preserving an outline choice made while detection was running. Both upstream
canvas-scaling tests and 15 workspace-security tests also passed.

Formatting, dependency layers, all-target Clippy for algo/UI/engine/ML, and
`cargo xtask wasm` passed. The final comparison-caption color correction was
checked by recompiling/linting all UI targets and inspecting fresh renders;
it did not change editing behavior. The final UI source also passed a separate
WASM check after the native-only type guard was cleaned up. Existing dead-code
warnings in CMS and engine remain outside this change.

Offscreen renders were inspected in all eight themes at 1280 x 900, with
detection disabled at 1024 x 768, and at 200% display scaling with 100% image
zoom and the landmark overlay enabled. The review caught and corrected hidden
feature groups, a garbled zoom label, and the Classic comparison-caption contrast.
Logs and public-domain QA captures remain local under `log/face-controls/` and
`plan/face-qa/`; the broader full-crate suites are not claimed as passing.

The native detector found 478 landmarks in the public-domain NASA/scikit-image
portrait. An opt-in integration test exercised the main shape sliders at both
extremes, verified that the field actually changed, and checked exact reset and
an unchanged document before Apply. This is one acceptance portrait, not a claim
of detection accuracy on every face or pose.

The standalone **24 MP release benchmark** (`bench_face_liquify`) measured a
five-region face edit over 1,505,004 field nodes: **155.8 ms with one worker**, and
**49.5 ms with four workers**, with bit-identical output. This measures field
construction, not the complete display/refinement/upload time. These are local
measurements, not a hardware-independent performance budget.

### Original import validation on 2026-10-10 (fef040c8)

The committed source at `fef040c8` passed all 11 validator gates on Windows with
Rust 1.99.0: formatting, authored-test discovery, 25 algorithm tests, 53 UI
tests, 14 engine distortion tests, dependency layers, Clippy across all targets
of the changed crates, WASM compatibility, the hostile-input command test, and
the native desktop build. All 57 package-authored Rust regressions executed.
The tested working tree matched the commit exactly.

An offscreen Windows render was inspected with a synthetic eye/grid fixture,
manual eye handles, and a 25% squint. The guide overlay, expression controls,
deformed preview, and undo availability rendered correctly. This is a synthetic
UI check rather than portrait acceptance testing.

The import also corrects three Clippy findings in the solver loops and boxes
large saved-rig fields to keep the UI state variants compact. The original
package's Python harness passed 63 of 69 tests on this Windows environment;
the other six require Unix-style Python/Bash commands or symlink privileges.
Those packaging-harness results are separate from the passing Rust gates.

Local logs are retained under `log/liquify-validation/`. They are deliberately
excluded from Git. The preserved Windows executable is
`dist/windows/photocraft.exe`. The broader full-crate test run was stopped when
the task was narrowed to a compact personal Windows build; it is not claimed as
passing. The automatic-face portrait regression and release field benchmark
described above were added afterward; the broader full-crate suites remain uncertified.

## Update from upstream

`origin` points to `voshart/photocraft`; `upstream` points to
`storytold/photocraft`. With a clean working tree:

```powershell
git fetch upstream
git switch main
git merge --ff-only upstream/main
git push origin main
git switch liquify/precision-pass4-validation
git merge main
```

Review any conflicts and rerun validation before publishing the updated feature
branch. Keep the lockfile pinned; updating upstream does not require upgrading
every third-party dependency independently. On Windows, run the original
package installer with `python -X utf8` so its Unicode replacement anchors are
decoded correctly.
