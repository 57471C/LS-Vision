# Settings

Settings are stored in `vision/config.local.json` (gitignored). The unlock password is `admin` and is not saved. Unlock is per browser tab.

## Camera

- Select the camera and its mode.
- **Display FPS**: 15, 20, 25, or 30. This is how often the preview canvas asks for a new JPEG. Capture stays at the camera's selected mode.
- Preview JPEGs fit in 1280×720. Snapshots used for barcode decode and detection stay full resolution.
- Focus, brightness, and contrast are stored as 0–100. Reset and Cancel restore the last loaded values. Save writes the camera block.
- Preview opens a window for the selected camera.

## Capture

- **Test mode**: after a kit IC loads, the operator page asks for an image file instead of reading the camera.
- **Barcode scan**: whole seconds from 1 to 60. The default is 8. This is the operator countdown.
- **Part detection**: a region of the frame (`detectRoi`: `x`, `y`, `w`, `h` as fractions from 0 to 1). Detection crops to it. Clear removes it.
- **Barcode**: a region of the frame (`barcodeRoi`: `x`, `y`, `width`, `height` as fractions from 0 to 1). The operator crop uses the same object. With no region, decode uses the full frame. The operator page can also draw this region on the live feed when Settings is unlocked.

## Inference EP

- The dropdown lists only providers this PC can run. CPU is always listed. DirectML is listed when a Direct3D 12 adapter is present. CUDA is listed when an NVIDIA adapter and the CUDA execution provider libraries load.
- The choice is saved as `inferenceEp`.
- If the selected provider fails, the session falls back to CPU and the header updates. The saved choice is kept.
- Benchmark times the active provider on a fixed image.

## Models

Training lists `.pt` and `.onnx` files in the app data models folder. Update model copies an `.onnx` file over the selected `.onnx` name, or over `best.onnx`. The selected basename is `model` in `vision/config.local.json`. Detection runs only for `.onnx`, and the next run loads the copied file.

## Part rules

Settings > Parts stores `partRules` in `vision/config.local.json`. Each row is a parent class, a child class, a condition (`contains` or `no child`), and a result IC. Add and remove rows, then Save. Up and Down set the order. Rules run from top to bottom and the first match wins.

`contains` matches when the child box center lies inside the parent box, including the edge. `no child` matches when it does not. The parent box is renamed to the result IC. The child box keeps the class the model gave it. An empty list leaves every class unchanged.

Example: parent `Hinge_Plate`, child `P02_Logo`, condition `contains`, result `2QHP02`. A second row with condition `no child` and result `2QHP01` names a plate that has no logo inside it.

## Database

- BoM source: JSON (`vision/sample-bom.json`), API (base URL, path, and API key; the barcode is the job card number), or MSSQL (Windows Authentication by default, SQL authentication optional).
- ERP button name and URL. Empty name and URL hides the operator button.

The operator page reloads the barcode ROI, Test mode, scan interval, BoM source, and ERP button when the window focuses.
