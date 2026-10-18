# Training and Models

## Current flow

1. Capture or collect images of kits/parts on the bench (JPG is fine).
2. Label in Label Studio (or a future built-in tool). Class names are part IC codes.
3. Export YOLO format.
4. Train with Ultralytics (or equivalent). Continue from an existing `best.pt` when adding classes.
5. Export to ONNX (`best.onnx`) and place in `vision/models/`.
6. Select the model and Inference EP in Settings.

## Compounding

You can continue training from `best.pt` as new part ICs are added. Keep the same Label Studio project and update `data.yaml` names list. Obsolete parts can be removed from the names list and the model retrained, or left in place if they no longer appear in BoMs.

## Test mode

When enabled, manually enter a kit IC and pick a local image. Useful for demos and regression checks without a live camera.
