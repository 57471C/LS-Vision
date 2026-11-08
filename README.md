# LS-Vision

Desktop application for verifying fastener kits on the bench using computer vision.

Built with Tauri 2, SvelteKit, TypeScript, and Rust. Version 0.5.2. Inference is ONNX Runtime in the Rust process. The camera, barcode decode, and BoM lookup run there too.

## What it does

Independent bench stations that:

- Read a single barcode from a fixed location on the bench
- Look up the Bill of Materials (BoM) for that job
- Detect and inspect the kits present
- Compare results against the expected parts and quantities
- Show clear pass/fail feedback

See [plan.md](plan.md) for the architecture and phased plan. Operator and admin notes are in [docs/](docs/README.md).

## Tech stack

- **Frontend**: SvelteKit + TypeScript + Vite
- **Desktop shell**: Tauri 2 (Rust)
- **Vision**: ONNX Runtime in Rust. The live preview is a paced canvas (Display FPS, JPEG scaled to fit 1280×720). A barcode snapshot is a separate full-resolution still, so the preview keeps drawing. Start/Stop Feed ignores extra clicks for 1.52 seconds. A `.pt` file is training-only and does not detect.
- **Inference EP**: Settings probes this PC and lists CPU, plus DirectML or CUDA when that runtime and GPU are present. This build links the OpenVINO prebuilt only, so the list is CPU. Use OpenVINO (Arc) registers the plugin beside the exe, or from `openvino/` when that folder is present, and loads the same ONNX files on the Arc GPU. A missing plugin stays on ONNX Runtime. A provider that fails to start falls back to CPU. The saved choice is kept.
- **Data**: JSON sample BoM, a job-card API, or MSSQL. The source is chosen in Settings.

## Getting started

```bash
npm install
npm run tauri dev
```

In Settings > Training, use Update model to copy an `.onnx` file into the app data models folder. Dev and the installed app share that folder. YOLO weight files are not stored in git and are not bundled. The installer bundles `models/dinov2_vits14_reg.onnx` and `models/dinov2_vitb14_reg.onnx` into `models/` next to the exe, and the OpenVINO DLLs into the same directory as the exe, when those files are present at build time. The non-register `dinov2_vits14.onnx` is not bundled. Settings > Training chooses `dinoModel` (`s_reg` or `b_reg`, default `s_reg`). Both stay gitignored. Training and export happen offline. The app loads `.onnx` files only. See [docs/Training-and-Models.md](docs/Training-and-Models.md).

## Project status

Version 0.5.2 runs inference, the camera, barcode decode, and BoM lookup in the Tauri process. A `.pt` model shows a warning instead of detecting.

Recent additions:

- A DINO catalogue import stores structured millimetres and each class's length and diameter rules. The overlay, the BoM line, and View references show the generated size, such as `M8x20` or `130x37 mm`. Metrology uses those fields. A Dimensions string is not parsed.
- Settings and the DINOv2 catalogue, including crop images, are stored in the results database (`results/results.sqlite`). Settings > Training chooses the DINOv2 model (`s_reg` or `b_reg`). The installer bundles both register models.
- After a barcode is accepted, the operator page waits a motion delay (default 400 ms) before detection. The overlay marks surplus, near-miss, and rogue parts for display only. Hovering a box shows its measured sizes.
- While detection runs, the operator camera and the camera preview show a green Detecting mark.
- Settings > Training > Decoder is Auto, YOLO, or RF-DETR. Auto reads the file name and the ONNX outputs. An RF-DETR export is stretched and ImageNet-normalised. A YOLO graph keeps the letterbox decoder.
- Settings > Camera > Preview, while admin is unlocked, draws a marquee on the detected still and saves that crop to the DINOv2 catalogue. A short press on a box still selects it. Crosshairs is optional, off by default, and shows the pointer in pixels and millimetres.
- Set scale and Measure draw a straight line with a 1px tick at each end. Saving camera settings keeps a stored scale and the detect regions when that save does not mention them.
- Settings > Training > Update model copies an `.onnx` file into `%APPDATA%\studio.lean.ls-vision\models`. Dev and the installed app share that folder. A file that is not ONNX is refused, and a missing model is reported on the operator page.
- The operator view draws a ROI marquee for part detection and for the barcode crop.
- The operator page is a shell around `OperatorToolbar`, `CameraView`, and `ResultsPanel`. Shared state is in `src/lib/stores/operator.ts`, and the handlers are in `src/lib/operator/session.ts`.
- ONNX inference resolves the model path and crops to the detect ROI before it runs.
- Settings lists the execution providers this PC can run. A provider that fails to start falls back to CPU, and the saved choice is kept.
