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
- Vision engine starts as Python sidecar, designed so it can later move to ONNX-in-Rust

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
- Move vision inference fully to ONNX-in-Rust  
- Separate training app if labeling needs grow  
- More advanced kit grouping if required

### Immediate next steps
2. Collect and label part and kit images in Label Studio.
3. Train and compare candidate models; export winning ONNX.
4. Wire detection results into the snapshot view.

This keeps the end-state in view while letting you prove the loop on one station first.
