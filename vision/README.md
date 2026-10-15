# Vision sidecar

Python sidecar for LS-Vision (OpenCV + Ultralytics YOLO).

This is the current vision engine. It is designed so inference can later move to ONNX-in-Rust without changing the contract with the Tauri app.

## Setup (first time)

```powershell
cd vision
python -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install -r requirements.txt
```

Place your best.pt in vision/models/. Weight files (`*.pt`, `*.onnx`) stay local and are not committed.

## Activate the environment (every new terminal)

```powershell
cd C:\Scripts\LS-Vision\vision
.\.venv\Scripts\Activate.ps1
```

You should see `(.venv)` in the prompt. Then you can run:

```powershell
python main.py
```

Health check: http://127.0.0.1:8008/health

## Notes

- Keep the API surface small and stable.
- BoM lookup and inspection results should stay easy to call from Rust/Tauri.
- Models will eventually be exported to ONNX for the Rust path.
- Sample BoM for testing: `sample-bom.json`
