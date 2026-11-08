# Changelog

## 0.5.2

- A DINO catalogue import stores structured millimetres with each class's length rule, diameter rule, and note. The operator overlay, the BoM line, and View references show the generated size, such as `M8x20` or `130x37 mm`. Metrology checks the fields those rules name. A Dimensions string is not parsed.
- Settings and the DINOv2 catalogue, including crop images, are stored in the results database. The first launch copies `vision/config.local.json` into that database when it has no settings. Settings > Training chooses the DINOv2 model, and the installer bundles both register models.
- After a barcode is accepted, detection waits for the motion delay. The overlay marks a match, a near miss, an extra, and an unknown part. Hovering a box shows its measured sizes.
- Preview Detect lists the closest catalogue matches. Append, and a marquee tick, save that crop under the item code the admin enters.

## 0.5.1

- Preview Detect uses the lower of the saved confidence and 0.30, and it draws boxes from every saved detect slot.
- A marquee tick in the camera preview appends that crop to the DINOv2 catalogue.

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

- Inference, the camera, barcode decode, and BoM lookup run in the Tauri process.
- The Windows installer targets Program Files.
