# AGENTS.md

Guidance for AI coding agents working on LS-Vision.

## Project context

LS-Vision is a Tauri 2 + SvelteKit desktop app for bench-top kit verification using computer vision. Version 0.3.3. The high-level plan lives in `plan.md`. Prefer small, incremental changes that move the core loop forward.

## Architecture notes

- Frontend: SvelteKit routes under `src/routes/`. `src/routes/+page.svelte` is a thin shell: it creates the operator session from `src/lib/operator/session.ts`, provides the store from `src/lib/stores/operator.ts`, and renders `OperatorToolbar`, `CameraView`, and `ResultsPanel`. Settings stays on `src/routes/settings/+page.svelte`.
- Rust/Tauri backend: `src-tauri/`.
- Inference is ONNX Runtime in Rust (`detect_image`). The app does not start a Python process. A `.pt` file is a training checkpoint: the page warns and does not detect.
- Settings lists the execution providers this PC can run. CPU is always listed. DirectML is listed when DirectML and a Direct3D 12 adapter are present. CUDA is listed when an NVIDIA adapter and the CUDA execution provider libraries load. The saved `inferenceEp` is used when the session is created. A provider that fails to start falls back to CPU. The saved choice is kept.
- The operator preview is a paced canvas. It calls `capture_preview` (scaled to fit 1280×720) and waits for the next frame until the draw finishes. The interval follows Display FPS. `capture_snapshot` copies a separate still handle at the camera's full resolution, so a barcode snapshot does not stall the preview slot.
- The existing camera preview window (Settings > Camera > Preview) zooms with Ctrl+wheel and pans with Space+left drag. A Focus slider on that preview sets manual focus on the camera, from 0 to 100, and saves it for that camera. Auto on that bar returns the camera to autofocus and clears the saved manual value. A missing focus leaves autofocus alone. Enter takes a full-resolution snapshot. A modal suggests the last saved name, and adds ` (n)` when that file already exists in today's folder, so a save never overwrites. The last name is stored in `vision/config.local.json`. Images go to the Camera page training folder, default `Documents/ls-vision/training` (Documents even when LocalAppData exists), inside `dd-mm-yyyy`. The preview does not draw a class list, thumbnails, or an ROI.
- Start/Stop Feed ignores extra clicks for 1.52 seconds. While locked, the button reads Please wait, is disabled, and draws at half opacity.
- Barcode decode crops to `barcodeRoi` when one is saved, then tries the native QR detector, ZXing (QR), and `decode_barcode` (QR and Code 39). One trailing slash is removed. BoM lookup is JSON (`vision/sample-bom.json`), the job-card API, or MSSQL. Test mode looks up the typed code and then detects on a chosen image file.
- BoM lookup stays behind a simple interface so Settings can swap JSON, API, and MSSQL.
- The operator Console button and drawer show only while admin is unlocked (`sessionStorage` key `ls-vision-settings-open`). Lines are still recorded when the page is locked. Locking admin closes the drawer.
- Release builds enable the Tauri `devtools` feature, and the main window and camera preview set `devtools: true`. Ctrl+Shift+I is Tauri's shortcut. F12 calls the same inspector from `src/routes/+layout.svelte`.
- Pass and fail inspections are stored in SQLite under LocalAppData (`results/results.sqlite`), or Documents when LocalAppData is missing. When detection ran, the stored pass flag also requires every BoM line's seen count to meet or exceed the expected quantity. The row includes the job card number from the scanned label, or from the BoM when that lookup returns one. The snapshot JPEG is a file under `results/images`. The database stores that path. Settings > Capture has "Do not save Pass / Fail results", on by default (`skipResultSave`). When it is on, the row and the JPEG are not written. File names are `P-{kitIC}-{DDMMYY}-{HH-MM-SS}.jpg` and `F-{kitIC}-{DDMMYY}-{HH-MM-SS}.jpg`. Operators open `/results` from the operator header and from the top of the settings rail. Delete shows only while admin is unlocked and removes the row and the JPEG.

## Conventions

- Keep operator-facing UI simple and clear (big pass/fail, minimal steps).
- Admin functionality (parts, cameras, BoM source, models) should be separable from the main operator flow.
- One kit type and one barcode per bench/job.
- Prefer explicit, readable code over clever abstractions at this stage.
- When adding vision features, keep the Rust ONNX path. Do not add a Python runtime process or a call to port 8008.
- The Windows installer targets Program Files (NSIS `perMachine`; the MSI already uses Program Files). Photos, snapshots, and logs go under LocalAppData, or Documents when LocalAppData is missing. Do not write them under Program Files. Training snapshots from the camera preview are the exception: they default to `Documents/ls-vision/training` even when LocalAppData exists, then a `dd-mm-yyyy` folder.
- ONNX models live in the Tauri app data `models` folder (`%APPDATA%\studio.lean.ls-vision\models` on Windows) for both `npm run tauri dev` and the installed app. The active filename stays in `vision/config.local.json`. Settings > Training > Update model copies an `.onnx` file there, makes that file the active model, and shows the destination path or the copy error. A file that fails the ONNX header check is rejected with "Selected file is not a valid ONNX model". The installer does not bundle `.onnx` files. A missing or failed model is reported on the operator page.
- Detection confidence is `confidence` in `vision/config.local.json`, from 0.15 to 0.80, default 0.45. Settings > Training saves it when it changes. `detect_image` uses it as the YOLO score threshold. Boxes at or below that score are dropped.

## Current behaviour

1. Barcode read from the saved barcode ROI, on a paced live preview.
2. BoM lookup from JSON, the job-card API, or MSSQL.
3. Inspection result on the operator view. PASS requires a loaded BoM and every BoM line's detected count at or above the expected quantity. A missing barcode, a missing BoM, or a short line is FAIL. Until detection finishes, the banner is incomplete. Kit found/expected stays an info line and does not set PASS.
4. Full-resolution snapshot for detection, without freezing the preview canvas.

See `plan.md` for the phased plan. Update this file if major architectural decisions change.
