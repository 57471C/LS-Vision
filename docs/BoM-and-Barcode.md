# BoM and Barcode

## Barcode

- Kit label is placed in a known location (ROI can be set later by an admin).
- Supported formats include QR and Code 39 (suffix `/` is stripped).
- A successful read that matches a BoM can trigger inspection.

## Bill of Materials

Source is selectable in Settings:

- Local JSON or CSV for testing.
- MSSQL (Windows Authentication by default; SQL auth optional).

Typical query pattern (see `vision/bom_queries.sql` or plan.md for the exact version in use):

- Kit name from `IR` where `ic = @kitIC`.
- Parts from `bm` joined to `IR`, excluding labour (`3%`) and packaging (`2P%`).

## Kit count

Informational only. One kit type per bench is the intended operating mode.
