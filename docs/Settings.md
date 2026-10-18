# Settings

Settings are stored in `config.local.json` (gitignored).

## Camera

- Select camera and mode.
- **Display FPS**: 15, 20, 25, 30. Controls how often the preview canvas asks for a new JPEG. Capture stays at the camera's native rate.
- Preview is scaled (typically 1280×720) for smooth display. Snapshots used for detection stay full resolution.

## Inference EP

- Dropdown lists only providers this PC can run (CPU always; DirectML if a Direct3D 12 adapter is present; CUDA if an NVIDIA adapter and the CUDA EP libraries are available).
- Choice is saved as `inferenceEp`.
- If the selected provider fails, the session falls back to CPU and the header updates.

## Other

- BoM source (JSON / CSV / MSSQL).
- Admin flag (unlocks extra controls such as ROI marquee when implemented).
- Model selection (when multiple models are present).
