# Getting Started

## What the app does

LS-Vision inspects kits on a bench. It reads a barcode (or QR), looks up the Bill of Materials, captures a still, runs object detection, and compares what it sees against the expected parts and quantities.

## Running the app

1. Launch the desktop app (or `npm run tauri dev` during development).
2. The Python sidecar is no longer used for inference. Detection runs via ONNX Runtime in the Rust backend.
3. Open Settings to choose camera, preview FPS, and Inference EP.

## First-time checks

- Camera appears in Settings and preview is smooth.
- Inference EP shows the providers available on this machine (CPU, DirectML, CUDA).
- A known kit barcode returns a BoM.
