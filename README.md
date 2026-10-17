# LS-Vision

Desktop application for verifying fastener kits on the bench using computer vision.

Built with Tauri 2, SvelteKit, TypeScript, and Rust. `.onnx` detection runs in Rust. The Python sidecar still serves the camera, barcode frames, `.pt` models, and the settings routes.

## What it does

Independent bench stations that:

- Read a single barcode from a fixed location on the bench
- Look up the Bill of Materials (BoM) for that job
- Detect and inspect the kits present
- Compare results against the expected parts and quantities
- Show clear pass/fail feedback

See [plan.md](plan.md) for the full architecture, flow, and phased implementation plan.

## Tech stack

- **Frontend**: SvelteKit + TypeScript + Vite
- **Desktop shell**: Tauri 2 (Rust)
- **Vision**: `.onnx` detection in Rust (ONNX Runtime). The Python sidecar still runs the camera, barcode frames, and `.pt` models.
- **Data**: JSON/CSV for development; MSSQL for production BoM lookup

## Getting started

```bash
npm install
npm run tauri dev
```

### Python sidecar

Dependencies live in `vision/`. Activate the virtual environment in any terminal where you run Python:

```powershell
cd vision
.\.venv\Scripts\Activate.ps1
```

(First-time setup: `python -m venv .venv` then `pip install -r requirements.txt`.)

Model files (`*.pt` / `*.onnx`) are not stored in git. Take them from the matching Forgejo release and place them in `vision/models/`.

See [vision/README.md](vision/README.md) for full sidecar details.

## Project status

`.onnx` part detection runs in the Tauri process. Camera capture, barcode frames, `.pt` fallback, and most settings routes still use the Python sidecar. See `plan.md` for the remaining ONNX migration steps.
