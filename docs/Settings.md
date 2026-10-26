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
- **Part detection**: up to four regions (`detectRois`, each `x`, `y`, `w`, `h` as fractions from 0 to 1, plus `enabled`). The Slot dropdown chooses which region the fields edit. Blank leaves the fields idle. An empty slot is skipped. With no slots saved, detection uses the full image. `detectRoi` is slot 1. Save writes the selected slot. A new box starts enabled. Clear removes that slot and packs the rest to the front, so the saved list has no gap. The fields then show whatever region now sits in the selected slot. The four squares flip `enabled` and save it immediately. Grey has no box, green is on, and red is off. Detection skips a slot that is off.
- **Barcode**: a region of the frame (`barcodeRoi`: `x`, `y`, `width`, `height` as fractions from 0 to 1). The operator crop uses the same object. With no region, decode uses the full frame. The operator page can also draw this region on the live feed when Settings is unlocked.

## Inference EP

- The dropdown lists only providers this PC can run. CPU is always listed. DirectML is listed when a Direct3D 12 adapter is present. CUDA is listed when an NVIDIA adapter and the CUDA execution provider libraries load. This OpenVINO prebuilt does not contain those providers, so DirectML is not listed and a CUDA choice still opens the CPU session.
- The choice is saved as `inferenceEp`.
- If the selected provider fails, the session falls back to CPU and the header updates. The saved choice is kept.
- **Use OpenVINO (Arc)** is `useOpenVino`, default off. Off keeps the ONNX Runtime provider above. On registers `onnxruntime_providers_openvino_plugin.dll`, or `onnxruntime_providers_openvino.dll`, from `openvino/` next to the exe, then beside the exe, then `resources/openvino`. It loads the same `.onnx` files on the OpenVINO GPU device. `openvino.dll` and the TBB runtime have to sit in that same folder. The installer places those DLLs beside `ls-vision.exe`. If the DLL is missing, registration fails, or the GPU session does not start, the session stays on ONNX Runtime and the status line says so. The switch stays saved.
- Benchmark times the active provider on a fixed image.

## Models

Training lists `.pt` and `.onnx` files in the app data models folder. Update model copies an `.onnx` file over the selected `.onnx` name, or over `best.onnx`. The selected basename is `model` in `vision/config.local.json`. Detection runs only for `.onnx`, and the next run loads the copied file.

Update references, under that control, opens a native file dialog that accepts `.json`. The chosen file replaces `vision/dinov2-references.json`, beside `vision/config.local.json`, and loads those embeddings. The status line is "References loaded N ICs." The next detection uses them. A missing file, a file that is not valid JSON, or a permission error stays on that status line. Cancel says "No file selected." The saved file and the in-memory map stay in place when the chosen file is refused.

View references, under that button, reads that same file and opens a table of each saved IC, its threshold, and how many embeddings it has. The dialog shows the file path. A missing or empty file says "No references loaded." The IC and threshold columns sort. Editing a threshold, or deleting an IC, writes `vision/dinov2-references.json` and the in-memory map. The next detection uses that change. An IC with no saved threshold uses the Training match score. Close returns to Settings.

## Part rules

Settings > Parts stores `partRules` in `vision/config.local.json`. Each row is a parent class, a child class, a condition (`contains` or `no child`), and a result IC. Parent and child are chosen from the active ONNX model's class list. Type in the field to filter that list. Add and remove rows, then Save. Up and Down set the order. Rules run from top to bottom and the first match wins.

`contains` matches when the child box center lies inside the parent box, including the edge. `no child` matches when that center is absent. A blank child on `no child` means any class inside the parent. The parent box is renamed to the result IC. The child box keeps the class the model gave it, and a child used by a matching contains rule is left out of the BoM count. The operator overlay hides that child. With admin unlocked, it is a thin grey outline. An empty list leaves every class unchanged. At inspect time, a rule whose parent or named child is not in the loaded model's class list is skipped. The operator page shows "Some part rules could not be applied (missing classes). Check Settings." and the operator console names each skipped rule. A model with no class metadata shows "This model has no class list." and leaves the class fields disabled. With no ONNX model, the message is "Load a model to choose classes."

Example: parent `Hinge_Plate`, child `P02_Logo`, condition `contains`, result `2QHP02`. A second row with condition `no child` and result `2QHP01` names a plate that has no logo inside it.

## Database

- BoM source: JSON (`vision/sample-bom.json`), API (base URL, path, and API key; the barcode is the job card number), or MSSQL (Windows Authentication by default, SQL authentication optional).
- **Description endpoint**: optional path stored as `api.descriptionPath`. The default is `/VisionAppApi/ListInventoryDescription`. Leave it blank to show an unexpected code by itself. A value is posted, with the API base URL and API key, when a detected code is not on the loaded BoM. The returned description is shown on the Not in BoM list.
- Results endpoint, optional and blank by default. Example: `/VisionAppApi/SubmitResult`. A saved inspection is posted to the API base URL plus this path, with the API key. Leave it blank to keep the result on this PC only. Test mode does not post.
- ERP button name and URL. Empty name and URL hides the operator button.

The operator page reloads the barcode ROI, Test mode, scan interval, BoM source, and ERP button when the window focuses.
