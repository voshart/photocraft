# Experimental precision Liquify

This fork carries the cumulative fourth-pass Liquify package on
`liquify/precision-pass4-validation`. The package was applied after synchronizing
`main` with `storytold/photocraft` at
`7722172585a01cbdb93c06f0f5ff2634fcb17999` on 2026-10-10. All four host source
blobs still matched the package's inspected baseline; the installer validated
100 replacements and 15 files before writing any changes.

The original ZIP, notes, and extracted package remain local and are excluded
through `.git/info/exclude`. They are not application dependencies.

Open **Filter > Liquify** to use Precision Push, source-detail previews,
manually positioned eye/mouth guides, expression controls, and Smart Filter
reopening. This remains experimental: a guide covers one region at a time;
automatic landmarks and persistent whole-face rigs are not included. Native
detail previews are bounded to a 4 MP visible region. Reopened Smart Filters
preview the isolated Liquify operation rather than the complete filter stack.

## Build and validation

Use Rust 1.95+ and the prerequisites in [development.md](development.md).

The personal Windows executable is `dist/windows/photocraft.exe`. Launch it
directly; Cargo is only needed when changing the source. Rebuild it with:

```powershell
./scripts/build-windows-personal.ps1
```

The script builds the native Windows app with two jobs, line-table debug symbols,
and incremental compilation disabled, then copies the executable and any runtime
DLLs into `dist/windows`. It does not build installers or release packages.
After copying the app, the generated `target` directory can be removed when
keeping the source tree compact matters more than faster incremental rebuilds.
The initial full debug/test cache reached 81.5 GB and was removed; the preserved
Windows executable is approximately 74 MB.

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

### Local validation on 2026-10-10

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
passing. Portrait acceptance tests and release performance benchmarks remain open.

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
