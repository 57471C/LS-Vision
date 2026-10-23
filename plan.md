**LS-Vision – High-level plan**

### Goal
Independent bench stations (NUCs) that:
- Read one barcode from a fixed square on the bench
- Pull the BoM (parts + qty) from MSSQL (JSON for testing)
- Detect the kits on the bench
- Verify every kit against that single BoM
- Show clear pass/fail (with per-kit detail)
- Support admin functions for parts, cameras, and settings

### Core principles
- One kit type per bench
- One barcode per job (in the drawn square)
- On-demand / slow capture is fine
- UI multi-camera capable from the start
- Admin tools live in the same app
- Vision engine is ONNX Runtime in Rust. The app does not start Python.

### Station flow
1. Operator places parts on bench for inspection
2. Operator places barcode label in the known square Region of Interest (ROI).
3. App reads the barcode from the fixed ROI (manual button still available as fallback)
4. Successful read triggers query BoM source (MSSQL or file) → expected parts + quantities, and expected kit/label count if available.
5. Detect kit regions on the bench.
6. For each kit: detect parts, measure critical dimensions (e.g. bolt length), compare to BoM.
9. Compare number of kits found vs expected count.
10. Display results: overall pass/fail + per-kit breakdown.

### Hardware
- **Camera**: Basler ace 2 a2A3536-31ucPRO (12.5 MP, IMX676, USB 3.0, C-mount) + Basler Lens C23-0828-16M (8 mm).
- **Working distance**: ~1200 mm covering the full 590 mm × 1400 mm scannable bench area.
- **Station PC**: ASUS NUC 16 Pro (RNUC16GDKX76C4N3) — Core Ultra X7 358H, Arc B390 iGPU, 32 GB RAM, 1 TB NVMe, Windows 11 Pro, dual 2.5 GbE.
- Inference runs on the NUC (OpenVINO / ONNX Runtime targeting the Arc iGPU).

### UI structure (Svelte)
- **Operator view** (`src/routes/+page.svelte` and related)  
  Camera feed(s), barcode status, big pass/fail, per-kit results, manual trigger button.  
  Snapshot of a successful barcode+BoM read replaces the live feed for operator feedback.

- **Admin section** (protected)  
  - Parts library + capture/label tools  
  - Camera / bench config (including barcode ROI and layout hints)  
  - BoM source settings (JSON/CSV ↔ MSSQL, Windows Auth default)  
  - Model management (import ONNX, view active model)

### Data & integration
- BoM lookup interface: `barcode → { des, expectedKitCount?, parts: [{ic, des, qty}] }`
- Lookup key = kit IC (e.g. `6PBBK001`)
- Pluggable source (MSSQL or JSON) via admin setting
- SQL queries live in a gitignored `vision/bom_queries.sql` (example committed)
- Results logging (local + optional central)

#### Example BoM JSON shape (Option A)
```json
{
  "6PBBK001": {
    "des": "Accessory Kit Bolt Bag 001",
    "expectedKitCount": 1,
    "parts": [
      { "ic": "2BBMNM08", "qty": 18, "des": "M8 Nut Nyloc z" }
    ]
  }
}
```

#### Example SQL (MSSQL)
Kit header + lines. Adjust table/column names to match the real schema.

```sql
-- Kit description
SELECT des
FROM IR
WHERE ic = @kitIc;          -- @kitIc = barcode payload, e.g. '6PBBK001'

-- Parts for the kit (already filtered: no 3*, no 2P* packaging, etc.)
SELECT
    p.ic,
    p.des,
    p.qty
FROM KitLines p             -- or BOM / IR lines table
WHERE p.kit_ic = @kitIc
  AND p.ic NOT LIKE '3%'
  AND p.ic NOT LIKE '2P%';
```

The app expects the combined result in the JSON shape above. `expectedKitCount` can come from a label-print count or be omitted.

### Vision / model exploration
Labelling with Label Studio (Community, Apache 2.0). Export YOLO or COCO format.

Candidate detectors to train and compare on the same labelled set:

| Model family | License | Notes |
|--------------|---------|-------|
| Ultralytics YOLOv8 / v11 (s or m) | AGPL-3.0 | Strong baseline, familiar workflow, excellent ONNX export |
| RT-DETR / RT-DETRv2 | Apache 2.0 | Strong accuracy on small objects, clean ONNX path |
| YOLOX | Apache 2.0 | Proven industrial option, good balance of speed and accuracy |

Train several options, compare on validation set (especially close variants), pick the best, export to ONNX for the NUC.

#### Fine-grained parts (example: M12×40 fine vs coarse)
- Treat as **separate classes** from the start (`M12x40-fine`, `M12x40-coarse`).
- Include clear close-up images where thread pitch/profile is visible.
- Train at higher resolution (1280) so fine detail is not lost.
- Use a small or medium model; nano is usually insufficient for thread discrimination.

### Implementation phases

**Phase 1 – Core loop (single camera, file BoM)**  
- Fixed barcode ROI read  
- JSON/CSV BoM lookup  
- Basic kit detection + part counting  
- Simple pass/fail on `+page.svelte`  
- Manual + auto-trigger from barcode

**Phase 2 – Real BoM + measurement**  
- MSSQL connection behind the same interface (Windows Authentication default)  
- Length/dimension checks for critical parts  
- Per-kit result display  
- Expected kit count cross-check

**Phase 3 – Admin & multi-camera ready**  
- Admin screens for parts, cameras, BoM source  
- Multi-camera settings (even if only one is used)  
- Basic labeling/capture for new parts  
- Model swap (drop in new ONNX)

**Phase 4 – Polish & rollout**  
- Debounce, operator feedback, error handling  
- Result images on failure  
- Deploy to additional NUCs  
- Optional central logging/dashboard

**Later**  
- Move vision inference fully to ONNX-in-Rust (see the migration notes below)  
- Separate training app if labeling needs grow  
- More advanced kit grouping if required

### ONNX-in-Rust migration

`.pt` stays the training file. Inference uses `.onnx`. Export with:

```text
yolo export model=best.pt format=onnx
```

Copy the `.onnx` file into the app data models folder with Settings > Training > Update model. Weight files stay out of git and out of the installer.

**Phase 1 (done on this branch).** `.onnx` detection runs in Rust through ONNX Runtime (`detect_image`). The operator page calls that command when the active model name ends in `.onnx`. An empty model selection uses `best.onnx` when that file is present, then `best.pt`. The detection JSON is unchanged: `{ detections: [{ ic, confidence, bbox }], image_base64 }`. The page still draws the boxes.

Python `YOLO(best.pt)` and `YOLO(best.onnx)` already disagree on `vision/test-images/Example3.jpg` (172 boxes from the checkpoint, 181 from the ONNX graph). Rust is checked against the ONNX result.

**Phase 2 (done on this branch).** The desktop app no longer starts `vision/main.py`, and the operator page no longer waits on port 8008.

- Camera: `list_cameras`, `list_camera_modes`, and `capture_snapshot` use Media Foundation on one worker thread. Only one camera is open. The previous media source is shut down before the next camera opens. Settings lists that camera's modes as size and fps. The saved value is `WIDTHxHEIGHT@FPS` (a blank value, or a size the camera does not list, uses the highest 16:9 mode at 30 fps, or the closest listed rate). The reader reopens in that exact mode and prefers NV12. 640×480 is used only when it is the saved mode or the only listed one. The source-reader callback requests the next sample before copying the current one. Each sample is stored as a preview handle and a still handle. `capture_preview` takes the preview handle. `capture_snapshot` copies the still handle and does not empty the preview slot, so a barcode snapshot does not freeze the preview canvas. Frames are not queued. JPEG encoding runs only when the frontend asks, off that callback. `capture_snapshot` returns a full-size JPEG of the selected mode plus `width`, `height`, `format`, and measured `fps`, for the operator page and for detection. `capture_preview` scales that same frame to fit in 1280×720 and returns a smaller JPEG. The Settings canvas asks for the next preview frame only after `drawImage` finishes. Display FPS is 15, 20, 25, or 30, default 15, and that choice is the target interval. The header shows the capture rate next to the canvas rate. The operator live view calls `capture_preview` and draws the next frame only after the previous `drawImage` finishes, at the saved Display FPS. Barcode reads and detection still call `capture_snapshot`. JPEG quality is 80. Media Foundation indexes can differ from the old DirectShow indexes, so a saved index may need to be chosen again. Focus, brightness, and contrast are stored in `vision/config.local.json` and are not applied to the device. A Settings slider change does not write the file; Save does. Choosing a resolution or a display rate writes it immediately. Switching cameras loads that camera's saved resolution, display rate, and levels. A camera with no saved record uses the defaults.
- Barcode: the page decodes the Rust JPEG with the native detector, then ZXing, then `decode_barcode`. It does not call the sidecar for a frame.
- BoM: JSON uses `load_bom_json`, the job-card API uses `post_bom_api`, and MSSQL uses `inspect_bom` (ODBC Driver 18, `Encrypt=optional;TrustServerCertificate=yes`). The SQL file is still `vision/bom_queries.sql`, with the example file as the fallback. The Rust path does not set the pyodbc `attrs_before` values. A missing kit is still `Barcode not found in BoM`.
- A `.pt` model does not detect. The page returns no boxes and the message `{name} is a .pt file. Choose an .onnx model to detect parts.` Choosing a model saves the basename in `vision/config.local.json`. Detection loads that `.onnx` file in Rust.
- Settings lists an Inference EP dropdown for the providers this PC can run. CPU is always listed. DirectML is listed when the DirectML runtime and a Direct3D 12 GPU are present. CUDA is listed when an NVIDIA GPU and the CUDA execution provider libraries load. The saved `inferenceEp` is used when the ONNX session is created, and a provider that fails to start falls back to CPU.
- The FastAPI sidecar is gone. `vision/main.py` is not in the repo, Settings has no Kill button, and the app does not call port 8008. Offline training still uses `vision/requirements.txt` (`opencv-python`, `ultralytics`, `numpy`, `pillow`).
- Part rules (`partRules` in `vision/config.local.json`, edited in Settings > Parts) run on the operator page after `detect_image` returns. A matching parent box is renamed to the result IC before the BoM count and the overlay. A child box used by that match is left out of the BoM comparison. Operators do not see its box. Admin sees a thin outline. A rule whose parent or named child is not in that model's class list is skipped, and the operator page shows a dismissible notice. The model output itself stays the trained class names. An empty list does not rewrite.

**Phase 3 (done).** The Python sidecar is off the runtime path. Training notes stay in `vision/README.md`. The README states that inference is ONNX Runtime in Rust. The model switcher, detect ROI, barcode ROI, and Settings write `vision/config.local.json` through Rust. Version 0.3.0.

### Immediate next steps
2. Collect and label part and kit images in Label Studio.
3. Train and compare candidate models; export winning ONNX.
4. Wire detection results into the snapshot view.

This keeps the end-state in view while letting you prove the loop on one station first.
