# Troubleshooting

## Preview is janky

- Check Settings preview first. If that is smooth, the Operator page may still be on the old frame path.
- Confirm Display FPS is set and the canvas is pacing (request next frame only after draw).
- Capture fps should stay high; only canvas fps is throttled.

## Inference EP missing

- DirectML requires a Direct3D 12 adapter and DirectML.dll.
- CUDA requires an NVIDIA adapter plus the CUDA execution provider libraries (not shipped in the static build).
- CPU is always available and is the fallback.

## BoM not found

- Confirm the barcode reads the expected kit IC.
- Check the BoM source setting and connection (MSSQL Windows Auth, JSON path, etc.).
- Verify the SQL excludes the correct prefixes (`3%`, `2P%`).

## Sidecar / port errors

The Python sidecar (`vision/main.py` on port 8008) is no longer used for detection. If you still see connection refused messages, an old code path is being called.
