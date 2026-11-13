# LS-Vision User Manual

Operator and admin guide for the LS-Vision kit inspection app.

## Contents

- [Getting Started](Getting-Started.md)
- [Operator View](Operator-View.md)
- [Settings](Settings.md)
- [Camera and Preview](Camera-and-Preview.md)
- [Inference EP](Inference-EP.md)
- [BoM and Barcode](BoM-and-Barcode.md)
- [Training and Models](Training-and-Models.md)
- [Troubleshooting](Troubleshooting.md)

These pages describe the 0.5.4 app. Inference, the camera, barcode decode, and BoM lookup run in Rust. Settings and the DINOv2 catalogue are stored in the results database. A catalogue part shows its generated size, such as `M8x20` or `130x37 mm`. The main detect keeps the highest DINOv2 match, then checks the BoM. Hovering a rejected box shows the top three catalogue guesses. The camera preview sets a per-camera scale, measures in millimetres, and, while admin is unlocked, captures a DINOv2 marquee and can show crosshairs. While detection runs, the camera shows a green Detecting mark.
