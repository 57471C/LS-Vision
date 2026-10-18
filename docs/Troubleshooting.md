# Troubleshooting

## Preview is janky

- Confirm Display FPS is 15, 20, 25, or 30 and that the canvas asks for the next frame only after the draw finishes.
- Capture stays at the camera mode. Canvas rate is limited by Display FPS and by encode and draw time.
- A barcode snapshot uses a separate still handle. The preview slot stays available, so the canvas should drop at most one frame when the countdown reaches 0.

## Start/Stop Feed feels stuck

The button ignores extra clicks for 1.52 seconds. It reads Please wait, stays disabled, and draws at half opacity until that window ends.

## Inference EP missing

- DirectML requires a Direct3D 12 adapter and DirectML.dll.
- CUDA requires an NVIDIA adapter plus the CUDA execution provider libraries. This build does not ship those libraries.
- CPU is always available and is the fallback. The saved provider is kept when fallback happens, so the next launch tries it again.

## A .pt model does not detect

Detection runs `.onnx` only. Export the checkpoint with `yolo export model=best.pt format=onnx`, then use Settings > Training > Update model to copy the file into the app data models folder.

## BoM not found

- Confirm the barcode reads the expected kit IC. A saved barcode ROI that misses the label will read nothing.
- Check the BoM source: JSON uses `vision/sample-bom.json`, the API uses the saved URL and key, and MSSQL uses the saved server and `vision/bom_queries.sql`.
- Verify the SQL excludes the intended prefixes (`3%`, `2P%`).

## Test mode

With Test mode on, the countdown does not open the camera. Type the kit IC and use the image prompt. Turn Test mode off to read the bench camera again.
