# Changelog

## 0.3.1

- Settings > Training > Update model copies an `.onnx` file into the AppData models folder and refuses a file that is not ONNX. A missing model is reported on the operator page.
- The operator view draws a ROI marquee for part detection and for the barcode crop.
- The operator page is a shell around OperatorToolbar, CameraView, and ResultsPanel. Shared state is in `src/lib/stores/operator.ts`, and the handlers are in `src/lib/operator/session.ts`.
- ONNX inference resolves the model path and crops to the detect ROI before it runs.
- Settings lists the execution providers this PC can run, keeps the saved choice, and falls back to CPU when a provider fails to start.

## 0.3.0

- Inference, the camera, barcode decode, and BoM lookup run in the Tauri process. The app does not start Python and does not call port 8008.
- The Windows installer targets Program Files.
