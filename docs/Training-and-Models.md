# Training and Models

## Current flow

1. Capture or collect images of kits/parts on the bench (JPG is fine).
2. Draw the boxes in [LS-Marker](https://github.com/57471C/LS-Marker). Class names are part IC codes.
3. Train the YOLO or COCO model in LS-Marker. Continue from an existing `best.pt` when adding classes.
4. Export to ONNX (`best.onnx`). Training and export happen offline. The app loads the `.onnx` file.
5. In Settings > Training, use Update model to copy that `.onnx` file into the app data models folder, then select the model and the Inference EP. A `.pt` selection warns and does not detect.

## DINOv2 model

Settings > Training, directly under Inference, chooses the catalogue matcher. `dinoModel` is `s_reg` (`dinov2_vits14_reg.onnx`, 384-dim CLS) or `b_reg` (`dinov2_vitb14_reg.onnx`, 768-dim CLS). A missing value is `s_reg`. The page shows the file name. Saving reloads the Rust session. Catalogue embeddings from the previous model are marked stale. The next extract opens the selected ONNX file and writes vectors of that model's CLS length only.

## Decoder

Settings > Training > Decoder is Auto, YOLO, or RF-DETR. Auto is the default. It uses the file name (`yolo`, `rfdetr`, `rf-detr`, or `incomplete`) and the ONNX output layout. A clear layout wins when the name disagrees. An RF-DETR export (`dets` and `labels`) is stretched to the model size and ImageNet-normalised. The saved confidence applies to both decoders. An export without NMS uses the same class-aware NMS as YOLO. A YOLO graph stays on the letterbox decoder. A file with no class-name metadata labels each box `class` plus the index, for example `class 4`.

## Compounding

You can continue training from `best.pt` as new part ICs are added. Add those classes in the same [LS-Marker](https://github.com/57471C/LS-Marker) project and train again. Obsolete parts can be removed from the class list and the model retrained, or left in place if they no longer appear in BoMs.

## Test mode

When enabled, manually enter a kit IC and pick a local image. Useful for demos and regression checks without a live camera.
