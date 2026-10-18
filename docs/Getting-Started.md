# Getting Started

## What the app does

LS-Vision inspects kits on a bench. It reads a barcode or QR, looks up the Bill of Materials, captures a still, runs object detection, and compares what it sees against the expected parts and quantities.

Inference is ONNX Runtime in the Rust backend. The app does not start a Python process.

## Running the app

1. Launch the desktop app, or run `npm run tauri dev` during development.
2. Open Settings and choose the camera, Display FPS, and Inference EP.
3. Put an `.onnx` model in `vision/models/` and select it. A `.pt` file does not detect.

## First-time checks

- The camera appears in Settings and the preview stays smooth.
- Inference EP lists the providers this machine can use (CPU, and DirectML or CUDA when they are available).
- A known kit barcode returns a BoM.
- Start/Stop Feed ignores extra clicks for 1.52 seconds.
