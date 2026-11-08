# Settings

Settings are stored in the `app_config` table of the results database (`results/results.sqlite` under LocalAppData, or Documents when that folder is missing). The unlock password is `admin` and is not saved. Unlock is per browser tab.

## Camera

- Select the camera and its mode.
- **Display FPS**: 15, 20, 25, or 30. This is how often the preview canvas asks for a new JPEG. Capture stays at the camera's selected mode.
- Preview JPEGs fit in 1280×720. Snapshots used for barcode decode and detection stay full resolution.
- Focus, brightness, and contrast are stored as 0–100. Reset and Cancel restore the last loaded values. Save writes the camera block. Brightness and contrast are saved with the camera and are not sent to it. The preview Focus slider sets focus on the open camera and saves it here. Auto on that slider clears the saved value. The same preview sets exposure and gain.
- Preview opens a window for the selected camera.

## Capture

- **Test mode**: after a kit IC loads, the operator page asks for an image file instead of reading the camera.
- **Barcode scan**: whole seconds from 1 to 60. The default is 8. This is the operator countdown. **Disable** (`barcodeScanDisabled`, default false) stops the camera barcode read, hides the barcode ROI on the operator camera, and focuses the code field. **Motion delay (ms)** is `motionDelayMs`, default 400, from 0 to 2000. After a barcode is accepted, the operator page waits that long before detection. A missing value is 400. The frozen frame stays as it is. Save writes the interval, the checkbox, and the delay together.
- **Part detection**: up to four regions (`detectRois`, each `x`, `y`, `w`, `h` as fractions from 0 to 1, plus `enabled`). The Slot dropdown chooses which region the fields edit. Blank leaves the fields idle. An empty slot is skipped. With no slots saved, detection uses the full image. `detectRoi` is slot 1. Save writes the selected slot. A new box starts enabled. Clear removes that slot and packs the rest to the front, so the saved list has no gap. The fields then show whatever region now sits in the selected slot. The four squares flip `enabled` and save it immediately. Grey has no box, green is on, and red is off. Detection skips a slot that is off.
- **Barcode**: a region of the frame (`barcodeRoi`: `x`, `y`, `width`, `height` as fractions from 0 to 1). The operator crop uses the same object. With no region, decode uses the full frame. The operator page can also draw this region on the live feed when Settings is unlocked.
- **Metrology**: off by default (`metrology.enabled`). When it is on, the margin, threshold mode (Otsu or fixed), fixed threshold, minimum contour area, and the length and diameter tolerances are shown. Save writes that object. Detection then measures each catalogue match on the full-resolution frame and can attach `pass` or `DIMENSION_MISMATCH`. The check needs a camera scale. The catalogue measurement method chooses the sizes. Expected millimetres come from the structured fields `thread`, `length_mm`, `af_mm`, `width_mm`, `height_mm`, `od_mm`, `id_mm`, and `thickness_mm`. `length_rule` and `diameter_rule` from the DINO catalogue name the length field and the diameter field. A missing rule uses the part type. A missing field leaves that side unset. A Dimensions string is not parsed. A bolt reports length under the head and thread diameter, a nut reports across flats, a washer reports outer diameter, inner diameter, and thickness, a cable tie reports total length and strap width, and a spring reports free height and outer coil diameter. Bolt head AF is kept when it is recorded and is not required. A missing method keeps length and diameter. The references list and the operator overlay show the generated size, for example `M6x30` or `130x37 mm`. The operator camera and the preview draw a thin arrowed line and a label for each span. A mismatch tints the line amber. Thickness does not decide the flag. The inspection pass or fail stays as it was.

## Inference EP

- The dropdown lists only providers this PC can run. CPU is always listed. DirectML is listed when a Direct3D 12 adapter is present. CUDA is listed when an NVIDIA adapter and the CUDA execution provider libraries load. This OpenVINO prebuilt does not contain those providers, so DirectML is not listed and a CUDA choice still opens the CPU session.
- The choice is saved as `inferenceEp`.
- If the selected provider fails, the session falls back to CPU and the header updates. The saved choice is kept.
- **Use OpenVINO (Arc)** is `useOpenVino`, default off. Off keeps the ONNX Runtime provider above. On registers `onnxruntime_providers_openvino_plugin.dll`, or `onnxruntime_providers_openvino.dll`, from `openvino/` next to the exe, then beside the exe, then `resources/openvino`. It loads the same `.onnx` files on the OpenVINO GPU device. `openvino.dll` and the TBB runtime have to sit in that same folder. The installer places those DLLs beside `ls-vision.exe`. If the DLL is missing, registration fails, or the GPU session does not start, the session stays on ONNX Runtime and the status line says so. The switch stays saved.
- Benchmark times the active provider on a fixed image.

## DINOv2 model

Directly under Inference, Training lists two DINOv2 files. The choice is `dinoModel` in `app_config`. `s_reg` is `dinov2_vits14_reg.onnx` (384 values) and `b_reg` is `dinov2_vitb14_reg.onnx` (768 values). A missing value is `s_reg`. The page shows the current file name. Changing it saves `dinoModel` and reloads the matcher. Rows already stored for the selected model are used again. When that model has no rows, the catalogue is marked stale. The next catalogue extract opens that ONNX file and writes vectors of its CLS length, so the two lengths are not mixed. Rows for the other model stay in the table. The matcher reads only the selected model. Vectors of a different length stay unused until that extract succeeds. The operator inspection and Preview Detect both use the loaded file.

## Models

Training lists `.pt` and `.onnx` files in the app data models folder. The path beside Update model opens that folder. Update model copies an `.onnx` file over the selected `.onnx` name, or over `best.onnx`. The selected basename is `model` in `app_config`. Detection runs only for `.onnx`, and the next run loads the copied file.

## Part rules

Update references, above the part rules, opens a native file dialog that accepts `.tar`. The archive contains `catalogue.json` and JPEG crops under `crops/`, including `crops/<IC>/<file>.jpg`. Each crop is embedded with the ONNX file named by the current `dinoModel`. The written vectors have that model's CLS length. Embeddings are written to `dino_catalogue` for the selected model. Part text and crop paths are written to `app_config`. The crop JPEG is stored in the `crop` column of that row. The next start loads that database.

The default import adds item codes from the tar and updates item codes that are already on this PC. Parts and crops that are only on this PC stay, including crops an admin added from the camera preview. Nothing that is missing from the tar is deleted. A non-empty description, type, or measurement method from the tar updates that field. An empty field leaves the saved value. Success says "Catalogue updated. N parts on this PC."

Fully replace catalogue is a checkbox on that same block. It is not saved. When it is checked, Update references opens a warning first. The warning says the import will replace the entire catalogue, and that admin-added crops and any parts not in the tar will be permanently removed. Replace continues to the file dialog and then installs the tar as the whole catalogue. The page shows the item code being extracted and how many crops are done. If a class has more crops than its limit, every crop is embedded and the catalogue database is written before that list appears, with a few crop thumbnails. A failed write shows the error and does not offer truncate or keep. Until that write succeeds, matching skips a stale catalogue. Truncate then removes the extra crops, using the newest when the tar has dates and a sample when it does not. Keep all stores the higher count for that class so a later full replace can use it. Cancel closes the warning or the file dialog and leaves the previous catalogue. Cancel on the crop list leaves the extracted catalogue in place. A corrupt archive or a failed embed still rejects the replace and leaves the previous catalogue. Success says "Catalogue loaded N parts." and, after a crop choice, adds which classes were truncated or kept in full.

Cancel on the file dialog says "No file selected." A bad archive, a missing DINOv2 model, or a write error leaves the saved catalogue in place and shows an error toast. The same is true when a merge cannot read a crop that is already on this PC.

View references lists every imported part and shows the generated size beside the description. Crops sit in a left-to-right wrap. A text field at the top filters by IC or description as you type. Each part shows its image count, for example "12 refs", and when the oldest crop was added. Click the IC to copy it. Identical files are marked Duplicate. Delete asks "Delete [IC] and all its reference images?" before it removes that part. A right-click on an image can reassign it or delete it, and several images can be selected and changed together. Drag an image to reorder it, or set one as primary. A tolerance beside a dimension is saved for that part in `app_config` and does not change the part rows. A missing or empty catalogue says "No references loaded." A filter with no hits says "No matches."

The parent and child rules stay below that control.

Settings > Parts stores `partRules` in `app_config`. Each row is a parent class, a child class, a condition (`contains` or `no child`), and a result IC. Parent and child are chosen from the active ONNX model's class list. Type in the field to filter that list. Add and remove rows, then Save. Up and Down set the order. Rules run from top to bottom and the first match wins.

`contains` matches when the child box center lies inside the parent box, including the edge. `no child` matches when that center is absent. A blank child on `no child` means any class inside the parent. The parent box is renamed to the result IC. The child box keeps the class the model gave it, and a child used by a matching contains rule is left out of the BoM count. The operator overlay hides that child. With admin unlocked, it is a thin grey outline. An empty list leaves every class unchanged. At inspect time, a rule whose parent or named child is not in the loaded model's class list is skipped. The operator page shows "Some part rules could not be applied (missing classes). Check Settings." and the operator console names each skipped rule. A model with no class metadata shows "This model has no class list." and leaves the class fields disabled. With no ONNX model, the message is "Load a model to choose classes."

Example: parent `Hinge_Plate`, child `P02_Logo`, condition `contains`, result `2QHP02`. A second row with condition `no child` and result `2QHP01` names a plate that has no logo inside it.

## Database

- BoM source: JSON (`vision/sample-bom.json`), API (base URL, path, and API key; the barcode is the job card number), or MSSQL (Windows Authentication by default, SQL authentication optional).
- **Description endpoint**: optional path stored as `api.descriptionPath`. The default is `/VisionAppApi/ListInventoryDescription`. Leave it blank to show an unexpected code by itself. The Not in BoM list uses the description stored on the matching catalogue part. A catalogue item code with no description there, and not on the loaded BoM, is posted with the API base URL and API key. The returned description is shown on that list. An unrecognized box is shown as its part type and is not posted.
- Results endpoint, optional and blank by default. Example: `/VisionAppApi/SubmitResult`. A saved inspection is posted to the API base URL plus this path, with the API key. Leave it blank to keep the result on this PC only. Test mode does not post.
- ERP button name and URL. Empty name and URL hides the operator button.

The operator page reloads the barcode ROI, Test mode, scan interval, BoM source, and ERP button when the window focuses.
