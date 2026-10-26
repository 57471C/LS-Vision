# Getting Started

## What the app does

LS-Vision inspects kits on a bench. It reads a barcode or QR, looks up the Bill of Materials, captures a still, runs object detection, and compares what it sees against the expected parts and quantities.

Inference is ONNX Runtime in the Rust backend. The app does not start a Python process.

## Running the app

1. Launch the desktop app, or run `npm run tauri dev` during development.
2. Open Settings and choose the camera, Display FPS, and Inference EP.
3. In Settings > Training, use Update model to copy an `.onnx` file into the app data models folder. A `.pt` file does not detect.

## First-time checks

- The camera appears in Settings and the preview stays smooth.
- Inference EP lists the providers this build can run. This OpenVINO build lists CPU. Use OpenVINO (Arc) loads the same ONNX files on the Arc GPU when the OpenVINO DLLs sit next to the exe.
- A known kit barcode returns a BoM.
- Start/Stop Feed ignores extra clicks for 1.52 seconds.
