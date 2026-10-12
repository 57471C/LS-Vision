from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel
import uvicorn
import json
from pathlib import Path

app = FastAPI(title="LS-Vision Sidecar")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

BOM_PATH = Path(__file__).parent / "sample-bom.json"

class InspectRequest(BaseModel):
    barcode: str

def load_bom():
    if not BOM_PATH.exists():
        return {}
    with open(BOM_PATH, encoding="utf-8") as f:
        return json.load(f)

@app.get("/health")
def health():
    return {"status": "ok", "engine": "python-sidecar"}

@app.post("/inspect")
def inspect(req: InspectRequest):
    bom = load_bom()
    job = bom.get(req.barcode)

    if not job:
        return {
            "barcode": req.barcode,
            "status": "fail",
            "message": "Barcode not found in BoM",
            "kits": []
        }

    return {
        "barcode": req.barcode,
        "status": "pass",
        "message": f"BoM loaded – {job.get('expectedKitCount', '?')} kits expected",
        "kitCode": job.get("kitCode"),
        "expectedKitCount": job.get("expectedKitCount"),
        "parts": job.get("parts", []),
        "kitsFound": job.get("expectedKitCount", 0),
        "kits": [
            {"id": i + 1, "status": "pass", "missing": [], "extra": []}
            for i in range(job.get("expectedKitCount", 1))
        ]
    }

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8008)