# Training and Models

## Current flow

1. Capture or collect images of kits/parts on the bench (JPG is fine).
2. Label in Label Studio (or a future built-in tool). Class names are part IC codes.
3. Export YOLO format.
4. Train with Ultralytics (or equivalent). Continue from an existing `best.pt` when adding classes.
5. Export to ONNX (`best.onnx`). The desktop app does not run the Python training environment.
6. In Settings > Training, use Update model to copy that `.onnx` file into the app data models folder, then select the model and the Inference EP. A `.pt` selection warns and does not detect.

## Decoder

Settings > Training > Decoder is Auto, YOLO, or RF-DETR. Auto is the default. It uses the file name (`yolo`, `rfdetr`, `rf-detr`, or `incomplete`) and the ONNX output layout. A clear layout wins when the name disagrees. An RF-DETR export (`dets` and `labels`) is stretched to the model size and ImageNet-normalised. The saved confidence applies to both decoders. An export without NMS uses the same class-aware NMS as YOLO. A YOLO graph stays on the letterbox decoder. A file with no class-name metadata labels each box with its class index.

## Compounding

You can continue training from `best.pt` as new part ICs are added. Keep the same Label Studio project and update `data.yaml` names list. Obsolete parts can be removed from the names list and the model retrained, or left in place if they no longer appear in BoMs.

## Test mode

When enabled, manually enter a kit IC and pick a local image. Useful for demos and regression checks without a live camera.
