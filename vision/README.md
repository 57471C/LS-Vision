# Offline training

Ultralytics is used to train and export models. The desktop app does not run this Python environment. Inference is ONNX Runtime in Rust.

## Setup (first time)

```powershell
cd vision
python -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install -r requirements.txt
```

`requirements.txt` pins the training stack: `opencv-python`, `ultralytics`, `numpy`, and `pillow`.

Place `best.pt` in `vision/models/`. Weight files (`*.pt`, `*.onnx`) stay local and are not committed.

`.pt` is the training checkpoint. Export it before the operator page can detect:

```powershell
yolo export model=best.pt format=onnx
```

Put the `.onnx` file in `vision/models/` and select it in Settings.

## Activate the environment (every new terminal)

```powershell
cd C:\Scripts\LS-Vision\vision
.\.venv\Scripts\Activate.ps1
```

## Notes

- Sample BoM for the JSON source: `sample-bom.json`
- Local settings, including the BoM source, live in gitignored `config.local.json`. Start from `config.example.json`.
- SQL lookup reads `bom_queries.sql` when that file is present, otherwise `bom_queries.example.sql`.
