# LS-Vision

Desktop application for verifying fastener kits on the bench using computer vision.

Built with Tauri 2, SvelteKit, TypeScript, and Rust. `.onnx` detection, the camera snapshot, barcode decode, and BoM lookup run in Rust. The app does not start the Python sidecar. `vision/main.py` is still in the repo.

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
- **Vision**: `.onnx` detection in Rust (ONNX Runtime). The camera snapshot is Rust (Media Foundation). A `.pt` file is training-only and does not detect.
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

`.onnx` detection, camera snapshots, barcode decode, and BoM lookup (JSON, API, and MSSQL) run in the Tauri process. The app does not spawn `vision/main.py`. That file remains for a manual run. A `.pt` model shows a warning instead of detecting. Settings can still stop a sidecar that was started by hand. See `plan.md` for Phase 3.
