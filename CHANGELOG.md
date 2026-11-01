# Changelog

## 0.5.0

- The operator camera and the camera preview show a green Detecting mark while detection runs.
- Settings > Training > Decoder runs YOLO or RF-DETR. Auto is the default and follows the file name and the ONNX outputs.
- Settings > Capture > Barcode scan > Disable stops the camera barcode read, hides the barcode ROI, and leaves the code field ready for a typed job card.
- A catalogue import merges with the parts already on this PC. Fully replace catalogue asks before it removes anything the archive does not contain.
- The camera preview sets UVC exposure, gain, and focus. Brightness and contrast stay in Settings and are not sent to the camera.
- Metrology follows the catalogue measurement method and draws a dimension line on each measured span. That result does not change the inspection pass or fail.

## 0.3.1

- Settings > Training > Update model copies an `.onnx` file into the AppData models folder and refuses a file that is not ONNX. A missing model is reported on the operator page.
- The operator view draws a ROI marquee for part detection and for the barcode crop.
- The operator page is a shell around OperatorToolbar, CameraView, and ResultsPanel. Shared state is in `src/lib/stores/operator.ts`, and the handlers are in `src/lib/operator/session.ts`.
- ONNX inference resolves the model path and crops to the detect ROI before it runs.
- Settings lists the execution providers this PC can run, keeps the saved choice, and falls back to CPU when a provider fails to start.

## 0.3.0

- Inference, the camera, barcode decode, and BoM lookup run in the Tauri process. The app does not start Python and does not call port 8008.
- The Windows installer targets Program Files.
