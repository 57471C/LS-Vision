import logging
import os
import sys
from pathlib import Path
from typing import Optional

def _windows_dll_dirs():
    # pyzbar loads libiconv.dll and libzbar-64.dll by name. Classic LoadLibrary
    # search uses PATH, not os.add_dll_directory, and a packaged exe inherits
    # Explorer's PATH rather than the dev shell's PATH.
    if os.name != "nt":
        return []
    exe_dir = Path(sys.executable).resolve().parent
    site = exe_dir.parent / "Lib" / "site-packages"
    found = []
    for path in (site / "pyzbar", site / "cv2", exe_dir, Path(__file__).resolve().parent):
        if path.is_dir() and path not in found:
            found.append(path)
    return found

def _prepare_windows_dlls():
    errors = []
    dirs = _windows_dll_dirs()
    if dirs:
        prefix = os.pathsep.join(str(path) for path in dirs)
        os.environ["PATH"] = prefix + os.pathsep + os.environ.get("PATH", "")
    for path in dirs:
        try:
            os.add_dll_directory(str(path))
        except (AttributeError, OSError) as exc:
            errors.append(f"add_dll_directory failed for {path}: {exc}")
    return dirs, errors

_DLL_DIRS, _DLL_SETUP_ERRORS = _prepare_windows_dlls()


import cv2
import numpy as np  # loaded after cv2 and before pyzbar, same point as before

def _prefer_bundled_dlls():
    # cv2 prepends its own bin directory while it imports. Put the bundled
    # barcode DLLs back at the front before pyzbar's LoadLibrary("libiconv.dll").
    if os.name != "nt" or not _DLL_DIRS:
        return
    prefix = os.pathsep.join(str(path) for path in _DLL_DIRS)
    os.environ["PATH"] = prefix + os.pathsep + os.environ.get("PATH", "")

_prefer_bundled_dlls()

# pyzbar's bundled libzbar needs MSVCR120.dll (VC++ 2013 x64). If that runtime
# is missing, keep /health and /inspect up and report the error from decode routes.
try:
    from pyzbar import pyzbar
except OSError as exc:
    pyzbar = None
    _PYZBAR_ERROR = str(exc)
else:
    _PYZBAR_ERROR = None

def _configure_logging():
    logger = logging.getLogger("ls-vision")
    logger.setLevel(logging.INFO)
    if logger.handlers:
        return logger
    formatter = logging.Formatter("%(asctime)s %(levelname)s %(message)s")
    logger.addHandler(logging.StreamHandler())
    try:
        file_handler = logging.FileHandler(
            Path(__file__).resolve().parent / "sidecar.log",
            encoding="utf-8",
        )
        logger.addHandler(file_handler)
    except OSError as exc:
        logger.warning("could not open sidecar.log: %s", exc)
    for handler in logger.handlers:
        handler.setFormatter(formatter)
    logger.propagate = False
    return logger

log = _configure_logging()

from fastapi import APIRouter, Body
from pydantic import BaseModel

# Fixed square on the bench. Change at runtime with POST /barcode/roi.
CAMERA_INDEX = 0
BARCODE_ROI = {"x": 0, "y": 0, "w": 640, "h": 480}

class BarcodeRequest(BaseModel):
    camera: Optional[int] = None
    x: Optional[int] = None
    y: Optional[int] = None
    w: Optional[int] = None
    h: Optional[int] = None

class RoiUpdate(BaseModel):
    camera: Optional[int] = None
    x: int
    y: int
    w: int
    h: int

def _resolve_roi(req):
    roi = dict(BARCODE_ROI)
    if req is None:
        return CAMERA_INDEX, roi
    camera = CAMERA_INDEX if req.camera is None else req.camera
    if req.x is not None:
        roi["x"] = req.x
    if req.y is not None:
        roi["y"] = req.y
    if req.w is not None:
        roi["w"] = req.w
    if req.h is not None:
        roi["h"] = req.h
    return camera, roi

def _crop_roi(frame, roi):
    height, width = frame.shape[:2]
    x = max(0, min(int(roi["x"]), max(width - 1, 0)))
    y = max(0, min(int(roi["y"]), max(height - 1, 0)))
    rw = max(1, min(int(roi["w"]), width - x))
    rh = max(1, min(int(roi["h"]), height - y))
    clamped = {"x": x, "y": y, "w": rw, "h": rh}
    return frame[y:y + rh, x:x + rw], clamped

def _clean_code(text):
    # Trailing "/" was a temporary Code 39 suffix. Revisit when labels move to QR.
    if text.endswith("/"):
        return text[:-1]
    return text

def _pyzbar_for_decode():
    # cv2 prepends its bin directory on import, and anything loaded later can
    # do the same. Restore the bundled barcode DLLs before every decode.
    global pyzbar, _PYZBAR_ERROR
    _prefer_bundled_dlls()
    if pyzbar is not None:
        return pyzbar
    try:
        from pyzbar import pyzbar as loaded
    except OSError as exc:
        pyzbar = None
        _PYZBAR_ERROR = str(exc)
        log.error("pyzbar failed to load: %s", _PYZBAR_ERROR)
        return None
    pyzbar = loaded
    _PYZBAR_ERROR = None
    log.info("pyzbar loaded")
    return pyzbar

def _collect_codes(decoded, origin_x=0, origin_y=0):
    results = []
    for obj in decoded:
        try:
            text = obj.data.decode("utf-8")
        except UnicodeDecodeError:
            text = obj.data.decode("latin-1", errors="replace")
        results.append({
            "type": obj.type,
            "data": _clean_code(text),
            "rect": {
                "x": obj.rect.left + origin_x,
                "y": obj.rect.top + origin_y,
                "w": obj.rect.width,
                "h": obj.rect.height,
            },
        })
    return results

def _log_decoded(codes):
    if codes:
        log.info(
            "barcode decoded %s",
            ", ".join(f"{item['type']} {item['data']}" for item in codes),
        )
        return
    log.info("barcode decode found no code")

def _scan_image(image, origin_x=0, origin_y=0):
    # pyzbar keeps only channel 0 of a colour array. A black label is still
    # visible there, but a mark that is dark in luminance and bright in blue
    # is not. Try grayscale first, then the original image.
    loaded = _pyzbar_for_decode()
    if loaded is None:
        return None
    views = []
    shape = getattr(image, "shape", ())
    if len(shape) == 3 and shape[2] >= 3:
        views.append(cv2.cvtColor(image, cv2.COLOR_BGR2GRAY))
    views.append(image)
    last_error = None
    for view in views:
        try:
            found = _collect_codes(loaded.decode(view), origin_x, origin_y)
        except Exception as exc:
            last_error = exc
            continue
        if found:
            return found
    if last_error is not None:
        raise last_error
    return []

def _decode(image):
    found = _scan_image(image)
    if found is None:
        log.warning("barcode decode failed: %s", _pyzbar_load_error())
        return []
    _log_decoded(found)
    return [{"data": item["data"], "type": item["type"]} for item in found]

def _pyzbar_load_error():
    return (
        "pyzbar failed to load libzbar. Install the Visual C++ 2013 x64 redistributable (MSVCR120.dll). "
        + (_PYZBAR_ERROR or "")
    )

def _codes_from_frame(frame):
    found = _scan_image(frame)
    if found is None:
        log.warning("barcode decode failed: %s", _pyzbar_load_error())
        return []
    _log_decoded(found)
    return found

router = APIRouter()

@router.get("/barcode/roi")
def get_barcode_roi():
    return {"camera": CAMERA_INDEX, "roi": BARCODE_ROI}

@router.post("/barcode/roi")
def set_barcode_roi(req: RoiUpdate):
    global CAMERA_INDEX, BARCODE_ROI
    if req.w <= 0 or req.h <= 0:
        return {"status": "fail", "message": "ROI width and height must be positive"}
    if req.camera is not None:
        CAMERA_INDEX = req.camera
    BARCODE_ROI = {"x": req.x, "y": req.y, "w": req.w, "h": req.h}
    return {"status": "pass", "camera": CAMERA_INDEX, "roi": BARCODE_ROI}

@router.post("/barcode")
def read_barcode(req: Optional[BarcodeRequest] = Body(default=None)):
    if _pyzbar_for_decode() is None:
        log.warning("barcode decode failed: %s", _pyzbar_load_error())
        return {
            "status": "fail",
            "barcode": None,
            "message": _pyzbar_load_error(),
            "codes": [],
        }

    camera, roi = _resolve_roi(req)
    if roi["w"] <= 0 or roi["h"] <= 0:
        return {
            "status": "fail",
            "barcode": None,
            "message": "ROI width and height must be positive",
            "camera": camera,
            "roi": roi,
            "codes": [],
        }

    from cameras import _capture_frame

    frame, error = _capture_frame(camera)
    if frame is None:
        return {
            "status": "fail",
            "barcode": None,
            "message": error,
            "camera": camera,
            "roi": roi,
            "codes": [],
        }

    try:
        crop, used_roi = _crop_roi(frame, roi)
        codes = _decode(crop)
    except Exception:
        log.exception("barcode read failed")
        return {
            "status": "fail",
            "barcode": None,
            "message": "Barcode decode failed",
            "camera": camera,
            "roi": roi,
            "codes": [],
        }

    frame_size = {"width": int(frame.shape[1]), "height": int(frame.shape[0])}
    if not codes:
        return {
            "status": "fail",
            "barcode": None,
            "message": "No barcode in ROI",
            "camera": camera,
            "roi": used_roi,
            "frame": frame_size,
            "codes": [],
        }

    return {
        "status": "pass",
        "barcode": codes[0]["data"],
        "type": codes[0]["type"],
        "message": "Barcode read",
        "camera": camera,
        "roi": used_roi,
        "frame": frame_size,
        "codes": codes,
    }
