# Vision sidecar

Python sidecar for LS-Vision (OpenCV + Ultralytics YOLO).

The desktop app no longer starts this process. `.onnx` detection, the camera snapshot, barcode decode, and BoM lookup run in Rust. `main.py` remains so the sidecar can still be started by hand, and so the training notes stay next to the export command.

## Setup (first time)

```powershell
cd vision
python -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install -r requirements.txt
```

Place your best.pt in vision/models/. Weight files (`*.pt`, `*.onnx`) stay local and are not committed.

`.pt` is the training file. The operator page runs `.onnx` in Rust. Export a model with:

```powershell
yolo export model=best.pt format=onnx
```

Put the `.onnx` file in `vision/models/` next to the `.pt` file.

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
- `.onnx` detection, the camera snapshot, and BoM lookup run in Rust. This process is not started by the app. A hand-started sidecar still answers on port 8008, and Settings can stop it.
- Sample BoM for testing: `sample-bom.json`
