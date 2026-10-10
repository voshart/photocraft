# photocraft-ml

L4 native inference, using the safe `mediapipe` Rust API and Google's official MediaPipe Tasks
0.10.35 runtime. No Python interpreter, browser, or web UI. `face_landmarks` accepts a bounded
sRGB thumbnail and returns normalized 478-point landmarks for one face. Editing
remains depth-independent in `photocraft-algo`; saved face edits replay without inference.

`scripts/setup-face-model.ps1` installs the checksum-pinned Windows model and runtime. The
runtime binding can also download its pinned official runtime when first used. It sends no
photo to a server. Native inference is excluded from WebAssembly builds.

Smoke test:

```powershell
cargo run -p photocraft-ml --example detect_faces -- dist/windows/models/face_landmarker.task image.png
```
