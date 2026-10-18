# Camera and Preview

## Preview path

- One latest-frame slot. The reader overwrites it on every sample.
- JPEG is encoded only when the canvas asks for a frame.
- Preview is scaled to fit 1280×720.
- Full-resolution snapshot remains available for detection and the operator snapshot button.
- Canvas paces itself: next frame is requested only after `drawImage` finishes, and the interval follows the Display FPS setting.

## Modes

Camera modes are enumerated from the device (MediaFoundation). Plugging in a new camera (4K, 12 MP, etc.) should list its supported modes automatically. No separate discovery script is required unless a device fails to enumerate.

## Troubleshooting

- If the Operator view is janky but Settings preview is smooth, the Operator page is still on the old path and needs to reuse `capture_preview`.
- Capture fps should stay near the camera's native rate (~30 fps for typical USB cameras). Canvas fps is limited by the Display FPS setting and encode/draw time.
