# Camera and Preview

## Preview path

- One Windows camera stays open on a single thread.
- Each sample is handed off once. A preview handle and a still handle share the media buffer. The callback does not copy the pixels. JPEG encoding runs only inside `capture_preview`.
- `capture_preview` takes the preview handle and encodes a JPEG that fits in 1280×720. The next preview waits for a new sample.
- `capture_snapshot` copies the still handle, converts it to RGB, and stores that frame. It does not empty the preview slot and it does not encode a JPEG. Conversion does not run on the camera thread.
- The canvas paces itself. The next frame is requested only after `drawImage` finishes, and the interval follows Display FPS (15, 20, 25, or 30).
- A barcode snapshot therefore does not freeze the preview. After a kit is held, the still image covers the canvas.

## Modes

Camera modes come from the device (Media Foundation). A newly attached camera lists the modes that device reports. Focus, brightness, and contrast are stored by Settings. This camera path does not apply those levels to the device.

## Settings preview

Settings can open a separate preview window for the selected camera. It uses the same paced `capture_preview` path and the same Display FPS.

Set scale sits in the preview header, next to the camera label, and only while admin is unlocked. The admin draws one straight line on the preview image. A dialog then asks for that length in millimetres. The factor is stored on that camera as `scale.mmPerPx`, with the width and height of the image, in `vision/config.local.json`. The header shows the current scale. Measure uses the same line. It shows the pixel length and the real length. With no scale stored, it shows the pixels and "No scale set for this camera." Locking admin hides Set scale and leaves Measure available.

Detect, on that window, stores the current raw frame and runs the active ONNX model through `detect_image`. The saved confidence threshold is the one the operator view uses. The button label is Detect. Boxes and a short list stay on the current preview frame until Clear or Escape. The \ key does the same while the preview window is focused. With admin unlocked, click one or more boxes and Save as reference. Append adds those crop embeddings under the YOLO class. Replace keeps only the selected crops for that class. Other classes stay. The vectors are written to `vision/dinov2-references.json` and loaded again on startup. The Save button is hidden while admin is locked. An unlocked admin can drag a temporary marquee on that still. The crop skips YOLO and is embedded with DINOv2. A small IC field, typed or picked from the model classes, plus Tick or X saves or discards that one vector. Append adds it. Replace keeps only that crop for the chosen IC. Other classes stay. The marquee is not saved as the bench ROI. It clears on X, Clear, Escape, a new Detect, or when admin locks. It does not look up a BoM or change the operator page. With no ONNX model loaded, the button is disabled and the window says "No model loaded."
