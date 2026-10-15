# LS-Vision

Desktop application for verifying fastener kits on the bench using computer vision.

Built with Tauri 2, SvelteKit, TypeScript, and Rust. Vision processing currently runs via a Python sidecar (OpenCV + YOLO), with a planned path to ONNX-in-Rust.

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
- **Vision (current)**: Python sidecar with OpenCV and Ultralytics YOLO
- **Vision (target)**: ONNX inference in Rust
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

Place your best.pt in vision/models/.

See [vision/README.md](vision/README.md) for full sidecar details.

## Project status

Early scaffolding. Core loop (barcode → BoM → inspection → result) is the next focus. See `plan.md` for phases.
