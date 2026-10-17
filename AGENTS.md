# AGENTS.md

Guidance for AI coding agents working on LS-Vision.

## Project context

LS-Vision is an early-stage Tauri 2 + SvelteKit desktop app for bench-top kit verification using computer vision. The high-level plan lives in `plan.md`. Prefer small, incremental changes that move the core loop forward.

## Architecture notes

- Frontend: SvelteKit routes under `src/routes/`. Operator UI starts at `src/routes/+page.svelte`.
- Rust/Tauri backend: `src-tauri/`.
- Vision sidecar (current): `vision/` Python package. `.onnx` detection runs in Rust (`detect_image`) and keeps the same JSON the page already draws. The sidecar still serves the camera, barcode frames, `.pt` models, and settings routes. Phase 2 in `plan.md` removes that sidecar.
- BoM lookup should stay behind a simple interface so the source (JSON/CSV vs MSSQL) can be swapped via settings.

## Conventions

- Keep operator-facing UI simple and clear (big pass/fail, minimal steps).
- Admin functionality (parts, cameras, BoM source, models) should be separable from the main operator flow.
- One kit type and one barcode per bench/job.
- Prefer explicit, readable code over clever abstractions at this stage.
- When adding vision features, consider both the current Python sidecar path and the future ONNX-in-Rust path.

## Current priorities

1. Barcode read from a fixed ROI.
2. BoM lookup (start with file-based).
3. Basic inspection result returned to the operator view.
4. Clear pass/fail display.

See `plan.md` for the full phased plan. Update this file if major architectural decisions change.
