**LS-Vision – High-level plan**

### Goal
Independent bench stations (NUCs) that:
- Read one barcode from a fixed square on the bench
- Pull the BoM (parts + qty) from MSSQL (JSON/CSV for testing)
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
1. Operator places barcode label in the known square and lays out the kits.
2. App reads the barcode from the fixed ROI.
3. Successful read triggers inspection (manual button still available as fallback).
4. Query BoM source (MSSQL or file) → expected parts + quantities, and expected kit/label count if available.
5. Detect kit regions on the bench.
6. For each kit: detect parts, measure critical dimensions (e.g. bolt length), compare to BoM.
7. Compare number of kits found vs expected count.
8. Display results: overall pass/fail + per-kit breakdown.

### UI structure (Svelte)
- **Operator view** (`src/routes/+page.svelte` and related)  
  Camera feed(s), barcode status, big pass/fail, per-kit results, manual trigger button.

- **Admin section** (protected)  
  - Parts library + capture/label tools  
  - Camera / bench config (including barcode ROI and layout hints)  
  - BoM source settings (JSON/CSV ↔ MSSQL)  
  - Model management (import ONNX, view active model)

### Data & integration
- BoM lookup interface: `barcode → { parts: [{code, qty, ...}], expectedKitCount? }`
- Pluggable source (file now, MSSQL later) via admin setting
- Results logging (local + optional central)

### Implementation phases

**Phase 1 – Core loop (single camera, file BoM)**  
- Fixed barcode ROI read  
- JSON/CSV BoM lookup  
- Basic kit detection + part counting  
- Simple pass/fail on `+page.svelte`  
- Manual + auto-trigger from barcode

**Phase 2 – Real BoM + measurement**  
- MSSQL connection behind the same interface  
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

### Immediate next steps (when you’re back at the machine)
1. Confirm barcode ROI approach and get a first read working from the camera.
2. Stub the BoM interface with a JSON file.
3. Wire a minimal inspection result back to `+page.svelte`.

This keeps the end-state in view while letting you prove the loop on one station first.