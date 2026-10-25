# Inference EP

ONNX Runtime execution providers used for detection. Inference runs in the Rust process.

## Available providers

| Provider | When it appears |
|----------|-----------------|
| CPU | Always |
| DirectML | DirectML.dll loads and a hardware Direct3D 12 adapter is present. This OpenVINO prebuilt does not include DirectML, so it is not listed. |
| CUDA | An NVIDIA adapter is present and the CUDA EP libraries load. This prebuilt has no CUDA provider, so a listed choice still opens the CPU session. |

## Behaviour

- Settings probes at startup and only lists what this machine can use.
- Preference order: DirectML > CUDA > CPU (unless a previously saved choice is still valid).
- Active provider is shown in the Settings header.
- Runtime failure falls back to CPU; the saved choice is kept so the next launch retries it.
- Use OpenVINO (Arc) is a separate switch, `useOpenVino`, default off. On looks for `onnxruntime_providers_openvino_plugin.dll`, then `onnxruntime_providers_openvino.dll`, in `openvino/` next to the exe, then beside the exe, then `resources/openvino`. It registers that file and loads the same `.onnx` files on that plugin's GPU device. YOLO and DINOv2 both use that session. `openvino.dll` and the TBB runtime have to sit in the same folder as the provider DLL. If the DLL is missing or registration fails, inference stays on the provider above and the Training status line says `OpenVINO is not available. Using ONNX Runtime.` If the GPU session does not start, the line says `OpenVINO did not start. Using ONNX Runtime.` The switch stays on so the next launch tries again. The linked ONNX Runtime build does not include OpenVINO. The installer resources are `models/*` and `openvino/*`, placed in those folders next to the exe. DINOv2 loads `models/dinov2_vits14.onnx` from that directory.

## Notes

This crate uses the OpenVINO prebuilt (`ndarray`, `download-binaries`, `copy-dylibs`, `openvino`). DirectML and CUDA are not in that runtime, so DirectML is not listed and a CUDA choice still opens the CPU session.
