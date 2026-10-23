# BoM and Barcode

## Barcode

- The kit label is read from `barcodeRoi` when that region is saved. Otherwise the full frame is used.
- Settings > Capture edits the region as fractions of the frame (left, top, width, height). The operator page can draw the same region on the live feed.
- Decode order on a camera still: native QR detector, then ZXing (QR), then Rust `decode_barcode` (QR and Code 39).
- One trailing slash is removed, so `6PBBK001/` matches `6PBBK001`.
- A successful read that matches the BoM holds the still and runs detection. The preview canvas is not stalled by that snapshot.

## Bill of Materials

The source is selected in Settings:

- **JSON**: `vision/sample-bom.json`, keyed by kit IC.
- **API**: POST of the job card number to the saved base URL and path, with header `X-Api-Key`. The response maps to the same kit IC, description, and parts.

## Inventory descriptions

Settings > Database has an optional description endpoint. It uses the same API base URL and `X-Api-Key`. It does not change the BoM lookup. The default path is `/VisionAppApi/ListInventoryDescription`. A blank path turns the lookup off.

When detection finds a code that is not on the loaded BoM, the app posts `{ "icList": ["..."] }` to that path. `Des` from the response is shown beside the code on the Not in BoM list, and it is stored with the saved result. If the path is blank, the base URL or key is missing, or the call fails, the list shows the code only.
- **MSSQL**: two parameterised queries from `vision/bom_queries.sql`, or `vision/bom_queries.example.sql` when the local file is absent. Windows Authentication is the default. SQL Server authentication stores the username and password in `vision/config.local.json`.

The usual query shape is the kit name from `IR` where `ic = @kitIC`, then parts from `bm` joined to `IR`, excluding labour (`3%`) and packaging (`2P%`).

## Test mode

Test mode skips the camera decode. The typed kit IC (or job card number, for the API source) is looked up, and a passing lookup asks for an image file to detect.

## Kit count

Informational only. One kit type per bench is the intended operating mode.
