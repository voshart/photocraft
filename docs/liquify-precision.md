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

```powershell
cargo run --release -p photocraft
python -X utf8 tools/liquify-validation/runner.py . --output log/liquify-validation/new-run
```

Choose a new output directory for each validation run. The runner retains
compiler, test, lint, WASM, and build results in `summary.json` and gate logs.
A native build alone does not establish that every check passed or that portrait
editing behaves correctly. Interactive portrait QA and performance comparisons
remain separate work.

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
