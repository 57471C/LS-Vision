import os
import sys
from contextlib import asynccontextmanager
from pathlib import Path

# Prepares the DLL path, imports cv2, then loads pyzbar before other imports.
import barcode
from barcode import _DLL_DIRS, _DLL_SETUP_ERRORS, _PYZBAR_ERROR, log, router as barcode_router

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
import uvicorn

from cameras import _shutdown_capture, router as camera_router
from detect import _ensure_yolo, router as detect_router
from config import router as config_router
from models import router as model_router
from bom import router as bom_router

def _log_runtime():
    for message in _DLL_SETUP_ERRORS:
        log.warning("%s", message)
    log.info(
        "sidecar start cwd=%s python=%s script=%s",
        os.getcwd(),
        sys.executable,
        Path(__file__).resolve(),
    )
    system = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "MSVCR120.dll"
    log.info("MSVCR120.dll %s", "found" if system.exists() else "missing")
    watched = ("libzbar-64.dll", "libiconv.dll", "opencv_videoio_ffmpeg500_64.dll")
    for directory in _DLL_DIRS:
        present = [name for name in watched if (directory / name).exists()]
        log.info(
            "dll dir %s has %s",
            directory,
            ", ".join(present) if present else "none of the camera or barcode dlls",
        )
    if _PYZBAR_ERROR:
        log.error("pyzbar failed to load: %s", _PYZBAR_ERROR)

_log_runtime()

@asynccontextmanager
async def _lifespan(_app):
    # Barcode and BoM stay up even when the weights fail to load.
    _ensure_yolo()
    try:
        yield
    finally:
        _shutdown_capture()

app = FastAPI(title="LS-Vision Sidecar", lifespan=_lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

@app.get("/health")
def health():
    return {"status": "ok", "engine": "python-sidecar"}

app.include_router(camera_router)
app.include_router(barcode_router)
app.include_router(detect_router)
app.include_router(config_router)
app.include_router(model_router)
app.include_router(bom_router)

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8008)
