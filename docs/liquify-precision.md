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

Hover over an eye, the mouth, nose, or face outline to reveal its on-image
handles. Small arrows show the drag direction; hovering a handle names the edit.
Eye handles adjust size, height and width, respecting the panel's linked-eye setting.
Hover at the center of either eye, the nose, or the mouth to reveal a four-arrow
**Move** handle. Drag it in any direction to reposition the whole feature; edge
handles keep their shape adjustments. Linked eyes move by the same offset so
their spacing stays unchanged. Uncheck **Adjust both eyes together** to move one
eye independently. Position sliders live under **More eye adjustments**, **Mouth
position**, and **Nose**. Zero restores the detected position. Movement is bounded
to 20% of the original feature width on each axis to keep the skin transition local.
The transition region expands smoothly with the move, and older saved edits
with separate eye adjustments retain independent eye movement.
Opening slider controls preserves saved fractional positions and head angles
until you edit them; hovering never rounds or changes the photo.
Mouth corners change smile and width; the top and bottom handles move the lip
outlines. Nose and face-outline handles change nose width/length and face
width, chin, forehead and jaw. Handles keep their screen size while zooming.
Hand, Zoom, Space/middle-button panning, and Original view retain navigation
priority. Select the face icon to return from Hand/Zoom to dragging face handles.
A drag changes the same absolute slider values, without brush strokes.

**Head rotation (2.5D)** provides **Turn left / right** and **Look down / up** in
degrees. Try 5–15° first; the combined yaw/pitch magnitude is capped at 30°.
Hover over the face outline and use **Turn head** above the forehead to drag
sideways and up/down. Reset face also resets rotation. Older saved face edits
without depth still work; Detect again enables rotation for them.

The detector's inferred Z coordinates are scaled by thumbnail width, following
[MediaPipe's weak-perspective coordinate specification](https://github.com/google-ai-edge/mediapipe/blob/master/docs/solutions/face_mesh.md#face-transform-module).
An original 25×25 depth surface drives orthographic rotation around a virtual
head pivot. Nose and cheek depths produce different movement. The image's inverse
map is solved on a 257×257 lattice, with positive-Jacobian and convergence checks,
then composed with the feature warp and freeze mask. A smooth outer transition
anchors the surrounding image. Depth and sliders are saved with the edit, so
replay does not need the detector. This is approximate relative rotation from
the current photograph, not a calibrated 3D reconstruction: it cannot reveal
occluded skin, hair, ears, or the far side of the nose. If an angle would fold the
face, the edit is rejected and the last accepted preview stays visible. The 30°
cap is a ceiling, not a promise that every face can turn that far; the acceptance
portrait supports ±15° and exercises safe rejection at unstable larger turns.

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

For the optimized personal Windows build used for this delivery, add `-Release`:

```powershell
./scripts/build-windows-personal.ps1 -Release
```

Both choices build only the Windows host app. Release compilation takes longer;
the resulting app uses optimized code. Neither choice creates installers or
packages for other operating systems.

The script builds the native Windows app with two jobs and incremental compilation
disabled. Debug builds retain line-table symbols; `-Release` uses the optimized
release profile. It copies the executable and any runtime
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

### Feature-center moves: validation (2026-10-10)

The final three-crate run passed **3,530 tests** across algo, engine, UI, their
integration tests, and doc tests, with 42 opt-in tests ignored and two confirmed
pre-existing failures explicitly excluded. The default run exposed a Windows
path-separator assertion in
`file_dialog::tests::save_dialogs_remember_where_each_document_was_last_saved`
and 34 missing Japanese catalog entries from the original Liquify package in
`i18n::tests::every_tl_literal_is_translated`. Both the save test and catalog are
unchanged from the previous commit; all missing literals already existed there.
The full default suite remains blocked by those failures; they were not disabled
or weakened in source.

The real-detector portrait test passes every position key at -100 and 100,
including independent right-eye movement, then resets without changing the
document. Other regressions cover coherent contour translation, preserved eye
spacing, original-width drag sensitivity, both drag axes, zoom, older unlinked
edits, center priority over a nearby retained edge handle, continuity at zero,
serialization/replay, invalid inputs, and 8/16/32-bit apply/undo. A GUI regression
also verifies that simply displaying sliders leaves fractional positions, shape
values, rotation, the field, and history unchanged. Visual review caught the
previous dynamic-angle slider rounding and the fixed mouth-region boundary;
both are corrected.

All-target Clippy for the three changed crates, layering, formatting, and the
full WebAssembly check passed. The hostile-input command test passed separately.
Eleven final offscreen captures cover both eye centers, nose and mouth centers,
linked and independent moves, nose/mouth translation, a combined feature/turn
edit, light theme, and 200% display scaling.

The 24 MP release field benchmark uses 1,505,004 nodes. These are individual
timings on this machine, excluding inference, display refinement, and upload:

| Workload | One worker | Four workers |
|---|---:|---:|
| Existing shape sliders | 158.4 ms | 52.7 ms |
| Shape sliders plus feature movement | 189.9 ms | 60.1 ms |
| Shape sliders plus 15-degree turn | 247.6 ms | 104.8 ms |
| Shape sliders, movement and 15-degree turn | 287.6 ms | 110.5 ms |

Each workload produces bit-identical output with one and four workers.
The broader `cargo xtask perf --quick` run passed. It uses smaller documents,
does not apply full-size budgets, and skipped regression comparison because
the committed baseline is from a different machine class. Existing unmeasured
or over-budget UI scenarios remain reported by the harness.

The final personal Windows build uses the optimized release profile and is
**65.77 MiB** at `dist/windows/photocraft.exe`; its version check succeeds.
It reports `dev build` because this fork is untagged, despite being optimized.

### Hover controls and depth turns: validation (2026-10-10)

After merging 90 upstream commits through `3092d3b6`, the focused checks passed:
**32 Liquify algorithm tests, 63 Liquify UI tests, 15 engine distortion tests,
one ML input test, one real-detector portrait test, and the hostile-input
command test**. Layers, formatting, all-target Clippy for algo/UI/engine/ML,
and WebAssembly compatibility passed. These are focused checks, not a claim
that every full-crate suite was run.

The real portrait supports yaw and pitch at ±15°. Larger turns either produce
an edit or return a fold/inversion error with the field, sliders, and history
unchanged. Regressions cover parallax, fixed background, bounded angles,
serialization/replay, freeze protection, direct mouse gestures, linked eyes,
zoom-independent values, navigation priority, and non-overlapping forehead/turn
handles. Visual review caught clipped angle values and overlapping handles;
both were corrected.
Selecting the face icon after Hand/Zoom returns to handle editing without a new
detection; this is covered by a full-dialog mouse-input regression.

The 24 MP release field benchmark measured **165.4 ms / 52.0 ms** for the
existing feature edit with one/four workers, and **245.5 ms / 99.7 ms** with a
15° depth turn added. Each uses 1,505,004 nodes; output is bit-identical between
one and four workers. This excludes inference, display refinement, and upload.
The broad `cargo xtask perf --quick` run also passed. Quick mode uses smaller
documents and does not enforce the full-size budgets; the baseline was from a
different machine class, so cross-machine regression comparison was skipped.
Existing over-budget/unmeasured UI scenarios remain reported by the harness.
That run preceded the final navigation-only correction, which passed the full
63-test Liquify UI set, native portrait regression, all-target UI Clippy, and a
separate UI WebAssembly check afterward. The final optimized Windows executable
is **65.75 MiB**; its version check succeeds. The build helper's `-Release`
option was exercised by that build.

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
