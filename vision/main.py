from typing import Optional

import cv2
from fastapi import Body, FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from pydantic import BaseModel
import uvicorn
import json
from pathlib import Path

# pyzbar's bundled libzbar needs MSVCR120.dll (VC++ 2013 x64). If that runtime
# is missing, keep /health and /inspect up and report the error from decode routes.
try:
    from pyzbar import pyzbar
except OSError as exc:
    pyzbar = None
    _PYZBAR_ERROR = str(exc)
else:
    _PYZBAR_ERROR = None

app = FastAPI(title="LS-Vision Sidecar")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

BOM_PATH = Path(__file__).parent / "sample-bom.json"
SAMPLE_DIR = Path(__file__).parent / "test-images"
DEFAULT_SAMPLE = "Example3.jpg"
# Areas within this multiple of the group median stay in the same size band.
SIZE_GROUP_RATIO = 2.0

# Fixed square on the bench. Change at runtime with POST /barcode/roi.
CAMERA_INDEX = 0
BARCODE_ROI = {"x": 0, "y": 0, "w": 640, "h": 480}

# One open capture shared by the stream and the decode calls.
camera = None
_camera_index = None

class InspectRequest(BaseModel):
    barcode: str

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

def load_bom():
    if not BOM_PATH.exists():
        return {}
    with open(BOM_PATH, encoding="utf-8") as f:
        return json.load(f)

def get_camera(index=0):
    global camera, _camera_index
    if camera is None or not camera.isOpened() or _camera_index != index:
        if camera is not None:
            camera.release()
        camera = cv2.VideoCapture(index, cv2.CAP_DSHOW)  # DSHOW is more reliable on Windows
        _camera_index = index
    return camera

def _open_camera(index):
    cap = cv2.VideoCapture()
    if hasattr(cv2, "CAP_PROP_OPEN_TIMEOUT_MSEC"):
        cap.set(cv2.CAP_PROP_OPEN_TIMEOUT_MSEC, 2000)
    if not cap.open(index, cv2.CAP_DSHOW):
        cap.open(index)
    return cap

def _capture_frame(index):
    cap = get_camera(index)
    if not cap.isOpened():
        return None, "Camera not available"
    frame = None
    # First frames from many webcams are blank. Keep the last good one.
    for _ in range(5):
        ok, grabbed = cap.read()
        if ok and grabbed is not None:
            frame = grabbed
    if frame is None:
        return None, "Failed to capture a frame"
    return frame, None

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

def _decode(image):
    if pyzbar is None:
        return []
    codes = []
    for barcode in pyzbar.decode(image):
        try:
            text = barcode.data.decode("utf-8")
        except UnicodeDecodeError:
            text = barcode.data.decode("latin-1", errors="replace")
        codes.append({
            "data": text,
            "type": barcode.type,
        })
    return codes

@app.get("/health")
def health():
    return {"status": "ok", "engine": "python-sidecar"}

@app.get("/camera/stream")
def camera_stream(index: int = 0):
    def generate():
        cap = get_camera(index)
        while True:
            ret, frame = cap.read()
            if not ret:
                break
            _, jpeg = cv2.imencode(".jpg", frame)
            yield (b"--frame\r\n"
                   b"Content-Type: image/jpeg\r\n\r\n" + jpeg.tobytes() + b"\r\n")
    return StreamingResponse(generate(), media_type="multipart/x-mixed-replace; boundary=frame")

@app.get("/camera/decode")
def camera_decode(index: int = 0):
    cap = get_camera(index)
    ret, frame = cap.read()
    if not ret:
        return {"status": "fail", "message": "No frame"}

    if pyzbar is None:
        return {
            "status": "fail",
            "message": "pyzbar failed to load libzbar. Install the Visual C++ 2013 x64 redistributable (MSVCR120.dll). " + (_PYZBAR_ERROR or ""),
            "codes": [],
        }

    decoded = pyzbar.decode(frame)
    results = []
    for obj in decoded:
        results.append({
            "type": obj.type,
            "data": obj.data.decode("utf-8"),
            "rect": {"x": obj.rect.left, "y": obj.rect.top, "w": obj.rect.width, "h": obj.rect.height}
        })

    return {
        "status": "pass" if results else "fail",
        "message": f"Found {len(results)} code(s)" if results else "No code detected",
        "codes": results
    }

@app.get("/cameras")
def cameras(count: int = 4):
    count = max(1, min(count, 8))
    available = []
    for index in range(count):
        if camera is not None and camera.isOpened() and _camera_index == index:
            available.append(index)
            continue
        cap = _open_camera(index)
        opened = cap.isOpened()
        cap.release()
        if opened:
            available.append(index)
    return {"cameras": available, "active": CAMERA_INDEX}

@app.get("/barcode/roi")
def get_barcode_roi():
    return {"camera": CAMERA_INDEX, "roi": BARCODE_ROI}

@app.post("/barcode/roi")
def set_barcode_roi(req: RoiUpdate):
    global CAMERA_INDEX, BARCODE_ROI
    if req.w <= 0 or req.h <= 0:
        return {"status": "fail", "message": "ROI width and height must be positive"}
    if req.camera is not None:
        CAMERA_INDEX = req.camera
    BARCODE_ROI = {"x": req.x, "y": req.y, "w": req.w, "h": req.h}
    return {"status": "pass", "camera": CAMERA_INDEX, "roi": BARCODE_ROI}

@app.post("/barcode")
def read_barcode(req: Optional[BarcodeRequest] = Body(default=None)):
    if pyzbar is None:
        return {
            "status": "fail",
            "barcode": None,
            "message": "pyzbar failed to load libzbar. Install the Visual C++ 2013 x64 redistributable (MSVCR120.dll). " + (_PYZBAR_ERROR or ""),
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

    crop, used_roi = _crop_roi(frame, roi)
    gray = cv2.cvtColor(crop, cv2.COLOR_BGR2GRAY)
    codes = _decode(gray)
    if not codes:
        codes = _decode(crop)

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

def _sample_path(name):
    if not name or Path(name).name != name or name.startswith("."):
        return None
    path = (SAMPLE_DIR / name).resolve()
    try:
        path.relative_to(SAMPLE_DIR.resolve())
    except ValueError:
        return None
    return path

def _contour_areas(image):
    gray = cv2.cvtColor(image, cv2.COLOR_BGR2GRAY)
    blur = cv2.GaussianBlur(gray, (7, 7), 0)
    dark = cv2.adaptiveThreshold(
        blur, 255, cv2.ADAPTIVE_THRESH_GAUSSIAN_C, cv2.THRESH_BINARY_INV, 41, 6
    )
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (5, 5))
    mask = cv2.morphologyEx(dark, cv2.MORPH_OPEN, kernel)
    mask = cv2.morphologyEx(mask, cv2.MORPH_CLOSE, kernel)
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)

    image_area = gray.shape[0] * gray.shape[1]
    areas = []
    for contour in contours:
        area = cv2.contourArea(contour)
        if area < image_area * 0.00015 or area > image_area * 0.02:
            continue
        _, _, width, height = cv2.boundingRect(contour)
        short_side = min(width, height)
        long_side = max(width, height)
        if short_side < 10 or long_side / float(short_side) > 8:
            continue
        if area / float(width * height) < 0.2:
            continue
        areas.append(area)
    areas.sort()
    return areas

def _group_by_size(areas):
    groups = []
    for area in areas:
        if not groups:
            groups.append([area])
            continue
        current = groups[-1]
        median = current[len(current) // 2]
        if area <= median * SIZE_GROUP_RATIO and area <= current[0] * (SIZE_GROUP_RATIO ** 2):
            current.append(area)
        else:
            groups.append([area])
    return groups

def _size_parts(groups):
    labels = ["S", "M", "L", "XL", "XXL"]
    descriptions = {
        "S": "Small",
        "M": "Medium",
        "L": "Large",
        "XL": "Extra large",
        "XXL": "Largest",
    }
    parts = []
    for index, group in enumerate(groups):
        label = labels[index] if index < len(labels) else f"S{index + 1}"
        name = descriptions.get(label, "Size group")
        low = int(round(group[0]))
        high = int(round(group[-1]))
        parts.append({
            "ic": label,
            "des": f"{name} blobs, about {low:,}–{high:,} px",
            "qty": len(group),
        })
    return parts

@app.post("/inspect/sample")
def inspect_sample(name: str = DEFAULT_SAMPLE):
    path = _sample_path(name)
    if path is None or not path.is_file():
        return {
            "status": "fail",
            "message": "Sample image not found",
            "kitCode": Path(name).name if name else DEFAULT_SAMPLE,
            "parts": [],
        }

    image = cv2.imread(str(path))
    if image is None:
        return {
            "status": "fail",
            "message": f"Could not read {path.name}",
            "kitCode": path.stem,
            "parts": [],
        }

    areas = _contour_areas(image)
    groups = _group_by_size(areas)
    parts = _size_parts(groups)
    if not parts:
        return {
            "status": "fail",
            "message": f"No blobs found in {path.name}",
            "kitCode": path.stem,
            "parts": [],
            "info": "Info only: classical contour count, not a BoM check.",
        }

    return {
        "status": "pass",
        "message": f"Rough size groups from {path.name}",
        "kitCode": path.stem,
        "parts": parts,
        "info": f"Info only: {len(areas)} blobs counted. Classical contour count, not a BoM check.",
    }

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

    kits_present = 2  # placeholder until real detection; change as needed for testing

    return {
        "barcode": req.barcode,
        "status": "pass",
        "message": f"BoM loaded – {kits_present} kit(s) on bench",
        "kitCode": job.get("kitCode"),
        "expectedKitCount": job.get("expectedKitCount"),  # display only
        "parts": job.get("parts", []),
        "kitsFound": kits_present,
        "kits": [
            {"id": i + 1, "status": "pass", "missing": [], "extra": []}
            for i in range(kits_present)
        ]
    }

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8008)
