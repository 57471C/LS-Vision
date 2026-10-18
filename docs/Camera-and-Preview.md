# Camera and Preview

## Preview path

- One Windows camera stays open on a single thread.
- Each sample is stored twice: a preview handle and a still handle.
- `capture_preview` takes the preview handle and encodes a JPEG that fits in 1280×720. The next preview waits for a new sample.
- `capture_snapshot` copies the still handle and does not empty the preview slot. The JPEG keeps the selected camera mode. Encoding does not run on the camera thread.
- The canvas paces itself. The next frame is requested only after `drawImage` finishes, and the interval follows Display FPS (15, 20, 25, or 30).
- A barcode snapshot therefore does not freeze the preview. After a kit is held, the still image covers the canvas.

## Modes

Camera modes come from the device (Media Foundation). A newly attached camera lists the modes that device reports. Focus, brightness, and contrast are stored by Settings. This camera path does not apply those levels to the device.

## Settings preview

Settings can open a separate preview window for the selected camera. It uses the same paced `capture_preview` path and the same Display FPS.
