# Inference EP

ONNX Runtime execution providers used for detection. Inference runs in the Rust process.

## Available providers

| Provider | When it appears |
|----------|-----------------|
| CPU | Always |
| DirectML | DirectML.dll loads and a hardware Direct3D 12 adapter is present (typical on Arc NUCs) |
| CUDA | NVIDIA adapter present and CUDA EP libraries load |

## Behaviour

- Settings probes at startup and only lists what this machine can use.
- Preference order: DirectML > CUDA > CPU (unless a previously saved choice is still valid).
- Active provider is shown in the Settings header.
- Runtime failure falls back to CPU; the saved choice is kept so the next launch retries it.
- Use OpenVINO (Arc) is a separate switch, `useOpenVino`, default off. On asks ONNX Runtime to load the same `.onnx` files with the OpenVINO execution provider on `GPU` (Arc). YOLO and DINOv2 both use that session. If this build has no OpenVINO provider, or the GPU session does not start, inference stays on the provider above and the Training status line says `OpenVINO is not available. Using ONNX Runtime.` or `OpenVINO did not start. Using ONNX Runtime.` The switch stays on so the next launch tries again.

## Notes

This build links ONNX Runtime statically and does not ship the CUDA runtime libraries. A driver-only NVIDIA PC will not list CUDA until those libraries are available.
