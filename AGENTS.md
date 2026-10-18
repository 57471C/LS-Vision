# AGENTS.md

Guidance for AI coding agents working on LS-Vision.

## Project context

LS-Vision is a Tauri 2 + SvelteKit desktop app for bench-top kit verification using computer vision. The high-level plan lives in `plan.md`. Prefer small, incremental changes that move the core loop forward.

## Architecture notes

- Frontend: SvelteKit routes under `src/routes/`. `src/routes/+page.svelte` is a thin shell: it creates the operator session from `src/lib/operator/session.ts`, provides the store from `src/lib/stores/operator.ts`, and renders `OperatorToolbar`, `CameraView`, and `ResultsPanel`. Settings stays on `src/routes/settings/+page.svelte`.
- Rust/Tauri backend: `src-tauri/`.
- Inference is ONNX Runtime in Rust (`detect_image`). The app does not start a Python process. A `.pt` file is a training checkpoint: the page warns and does not detect.
- Settings lists the execution providers this PC can run. CPU is always listed. DirectML is listed when DirectML and a Direct3D 12 adapter are present. CUDA is listed when an NVIDIA adapter and the CUDA execution provider libraries load. The saved `inferenceEp` is used when the session is created. A provider that fails to start falls back to CPU. The saved choice is kept.
- The operator preview is a paced canvas. It calls `capture_preview` (scaled to fit 1280×720) and waits for the next frame until the draw finishes. The interval follows Display FPS. `capture_snapshot` copies a separate still handle at the camera's full resolution, so a barcode snapshot does not stall the preview slot.
- Start/Stop Feed ignores extra clicks for 1.52 seconds. While locked, the button reads Please wait, is disabled, and draws at half opacity.
- Barcode decode crops to `barcodeRoi` when one is saved, then tries the native QR detector, ZXing (QR), and `decode_barcode` (QR and Code 39). One trailing slash is removed. BoM lookup is JSON (`vision/sample-bom.json`), the job-card API, or MSSQL. Test mode looks up the typed code and then detects on a chosen image file.
- BoM lookup stays behind a simple interface so Settings can swap JSON, API, and MSSQL.

## Conventions

- Keep operator-facing UI simple and clear (big pass/fail, minimal steps).
- Admin functionality (parts, cameras, BoM source, models) should be separable from the main operator flow.
- One kit type and one barcode per bench/job.
- Prefer explicit, readable code over clever abstractions at this stage.
- When adding vision features, keep the Rust ONNX path. Do not add a Python runtime process or a call to port 8008.
- The Windows installer targets Program Files (NSIS `perMachine`; the MSI already uses Program Files). Photos, snapshots, and logs go under LocalAppData, or Documents when LocalAppData is missing. Do not write them under Program Files.
- ONNX models live in the Tauri app data `models` folder (`%APPDATA%\studio.lean.ls-vision\models` on Windows) for both `npm run tauri dev` and the installed app. The active filename stays in `vision/config.local.json`. Settings > Training > Update model copies an `.onnx` file there. The installer does not bundle `.onnx` files.

## Current behaviour

1. Barcode read from the saved barcode ROI, on a paced live preview.
2. BoM lookup from JSON, the job-card API, or MSSQL.
3. Inspection result on the operator view, with a clear pass/fail.
4. Full-resolution snapshot for detection, without freezing the preview canvas.

See `plan.md` for the phased plan. Update this file if major architectural decisions change.
