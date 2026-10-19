# LS-Vision

Desktop application for verifying fastener kits on the bench using computer vision.

Built with Tauri 2, SvelteKit, TypeScript, and Rust. Version 0.3.2. Inference is ONNX Runtime in the Rust process. The camera, barcode decode, and BoM lookup run there too. The app does not start Python.

## What it does

Independent bench stations that:

- Read a single barcode from a fixed location on the bench
- Look up the Bill of Materials (BoM) for that job
- Detect and inspect the kits present
- Compare results against the expected parts and quantities
- Show clear pass/fail feedback

See [plan.md](plan.md) for the architecture and phased plan. Operator and admin notes are in [docs/](docs/README.md).

## Tech stack

- **Frontend**: SvelteKit + TypeScript + Vite
- **Desktop shell**: Tauri 2 (Rust)
- **Vision**: ONNX Runtime in Rust. The live preview is a paced canvas (Display FPS, JPEG scaled to fit 1280×720). A barcode snapshot is a separate full-resolution still, so the preview keeps drawing. Start/Stop Feed ignores extra clicks for 1.52 seconds. A `.pt` file is training-only and does not detect.
- **Inference EP**: Settings probes this PC and lists CPU, plus DirectML or CUDA when that runtime and GPU are present. A provider that fails to start falls back to CPU. The saved choice is kept.
- **Data**: JSON sample BoM, a job-card API, or MSSQL. The source is chosen in Settings.

## Getting started

```bash
npm install
npm run tauri dev
```

In Settings > Training, use Update model to copy an `.onnx` file into the app data models folder. Dev and the installed app share that folder. Weight files are not stored in git and are not bundled in the installer. See [vision/README.md](vision/README.md) for the offline training and export steps.

## Project status

Version 0.3.2 runs inference, the camera, barcode decode, and BoM lookup in the Tauri process. There is no Python sidecar and no port 8008 client. A `.pt` model shows a warning instead of detecting.

Recent additions:

- Settings > Training > Update model copies an `.onnx` file into `%APPDATA%\studio.lean.ls-vision\models`. Dev and the installed app share that folder. A file that is not ONNX is refused, and a missing model is reported on the operator page.
- The operator view draws a ROI marquee for part detection and for the barcode crop.
- The operator page is a shell around `OperatorToolbar`, `CameraView`, and `ResultsPanel`. Shared state is in `src/lib/stores/operator.ts`, and the handlers are in `src/lib/operator/session.ts`.
- ONNX inference resolves the model path and crops to the detect ROI before it runs.
- Settings lists the execution providers this PC can run. A provider that fails to start falls back to CPU, and the saved choice is kept.
