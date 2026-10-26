# Operator View

The main screen used on the bench.

## Live view

- The preview is a canvas. It calls `capture_preview` and requests the next JPEG only after the draw finishes.
- The pace follows Display FPS. Preview JPEGs are scaled to fit 1280×720. The camera keeps its selected mode.
- The header shows the capture size and the canvas rate.
- **Start feed** and **Stop feed** turn that preview on and off. Extra clicks are ignored for 1.52 seconds. While locked, the button reads Please wait, is disabled, and draws at half opacity.
- **Measure**, beside Camera, draws a straight line on the live preview or the held snapshot. The line shows its pixel length. When that camera has a scale from Settings > Camera > Preview, it also shows the length in millimetres. With no scale, it says "No scale set for this camera." Measure does not change the scale.

## Barcode and snapshot

- With Test mode off, the countdown reads a full-resolution raw frame (`capture_snapshot`) when it reaches 0. That still is a separate camera handle, so the preview canvas keeps drawing through the capture.
- Rust crops that frame to the saved barcode ROI when one is set. The page first tries the native detector and ZXing on the preview JPEG, then Rust (`decode_barcode`, QR and Code 39). A crop whose long edge is over 1920 pixels is halved until it fits, and that smaller image is read first. One trailing slash is removed.
- A code that matches the BoM holds the current preview JPEG over the canvas and runs detection on the raw frame. Boxes and IC labels are drawn on top of that preview. When more than one detect slot returned boxes, every slot is drawn, coloured and tagged with that slot number. One slot keeps the usual part colours. The preview slot was not cleared by the snapshot.
- Clear ROI removes the selected slot and packs the remaining regions to the front. The slot outlines and the result panels use that numbering at once. With no slots left, detection uses the full frame.
- While Settings is locked, four squares sit beside Camera when at least one slot has a box. With no saved slot, the squares stay hidden. Grey has no box and does not click, and the pointer stays a normal arrow. Green is on. Red is off. Keys 1–4 toggle a saved slot. Backtick turns every saved slot off. The choice is saved immediately and the next inspection uses it. Detection runs only on slots that are on. When every saved slot is off, the region outlines stay hidden and detection does not use the full frame. Those squares do not move or resize a box.
- **Decode QR / Barcode** uses the same decode path on demand.
- A miss, or a code that is not in the BoM, leaves the live view in place.

## Test mode

When Test mode is on, the countdown does not read the camera. The button reads **Use typed code**. A passing lookup asks for an image file and shows that file. Detection runs on that file.

## Results

The right-hand panel shows pass or fail, the kit, and the parts table (expected quantity against what detection saw).

Part rules, when Settings has any, run after the model returns boxes and before those counts and the boxes on the still. A matching parent class is shown as the result IC. A child box used by that match is left out of the BoM count and the pass check, so it does not show as a missing or extra part. The operator overlay hides that child. With admin unlocked, it is a thin grey outline. The result IC keeps its normal box. A rule whose parent or named child is not in that model's class list is skipped. The other rules still run, and the pass or fail result stays on screen. The page then shows a dismissible notice: "Some part rules could not be applied (missing classes). Check Settings." The operator console lists each rule that matched and each rule that was skipped. With no rules, the classes on screen are the ones the model returned.

## ERP panel

When Settings has an ERP name and URL, an amber button slides that page over the vision view. A strip on the left returns to the camera.
