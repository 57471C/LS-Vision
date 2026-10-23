# Operator View

The main screen used on the bench.

## Live view

- The preview is a canvas. It calls `capture_preview` and requests the next JPEG only after the draw finishes.
- The pace follows Display FPS. Preview JPEGs are scaled to fit 1280×720. The camera keeps its selected mode.
- The header shows the capture size and the canvas rate.
- **Start feed** and **Stop feed** turn that preview on and off. Extra clicks are ignored for 1.52 seconds. While locked, the button reads Please wait, is disabled, and draws at half opacity.

## Barcode and snapshot

- With Test mode off, the countdown reads a full-resolution still (`capture_snapshot`) when it reaches 0. That still is a separate camera handle, so the preview canvas keeps drawing through the capture.
- The page crops the still to the saved barcode ROI when one is set, then decodes QR with the native detector, then ZXing, then Rust (`decode_barcode`, QR and Code 39). One trailing slash is removed.
- A code that matches the BoM holds that still over the canvas and runs detection on it. Boxes and IC labels are drawn on top of the still. The preview slot was not cleared by the snapshot.
- **Decode QR / Barcode** uses the same decode path on demand.
- A miss, or a code that is not in the BoM, leaves the live view in place.

## Test mode

When Test mode is on, the countdown does not read the camera. The button reads **Use typed code**. A passing lookup asks for an image file and detects on that file.

## Results

The right-hand panel shows pass or fail, the kit, and the parts table (expected quantity against what detection saw).

Part rules, when Settings has any, run after the model returns boxes and before those counts and the boxes on the still. A matching parent class is shown as the result IC. The operator console lists each rule that matched. With no rules, the classes on screen are the ones the model returned.

## ERP panel

When Settings has an ERP name and URL, an amber button slides that page over the vision view. A strip on the left returns to the camera.
