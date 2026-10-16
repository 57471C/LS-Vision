import base64
import threading
from typing import Optional

import cameras
import cv2
import numpy as np
from cameras import _jpeg_base64
from fastapi import APIRouter, Body
from pydantic import BaseModel

from config import DEFAULT_MODEL_NAME, _clean_detect_roi, load_bom_source
from models import _model_choice, _model_file

yolo_model = None
_yolo_lock = threading.RLock()
# (path, mtime_ns) of the weights object in yolo_model. None when nothing is loaded.
_yolo_loaded_key = None
# (stored name, resolved name, mtime_ns) of the last load attempt, including failures.
_yolo_attempt_key = None
_yolo_message = ""

class DetectRequest(BaseModel):
    # data-url or raw base64. Omit it to use the latest snapshot.
    image: Optional[str] = None

def _short_error(exc):
    text = " ".join(str(exc).split())
    if len(text) > 240:
        return text[:237] + "..."
    return text or "unknown error"

def _load_weights(path):
    from ultralytics import YOLO
    loaded = YOLO(str(path))
    # First predict pays the device setup cost. Do it before the operator clicks.
    loaded.predict(np.zeros((64, 64, 3), dtype=np.uint8), verbose=False)
    return loaded

def _finish_load(path, note):
    loaded = _load_weights(path)
    try:
        mtime = path.stat().st_mtime_ns
    except OSError:
        mtime = None
    names = getattr(loaded, "names", {})
    labels = list(names.values()) if isinstance(names, dict) else list(names)
    print(f"Loaded YOLO model {path.name} with {len(labels)} part classes")
    return loaded, path, mtime, note

def _load_choice(path, note):
    # path is an existing weights file, or None when the choice is missing.
    if path is None:
        return None, None, None, note or f"YOLO model {DEFAULT_MODEL_NAME} was not found."
    try:
        return _finish_load(path, note)
    except Exception as exc:
        print(f"YOLO model {path.name} failed to load: {_short_error(exc)}")
        default_path = _model_file(DEFAULT_MODEL_NAME)
        if default_path is not None and default_path.resolve() != path.resolve():
            fallback = f"{path.name} failed to load. Using {default_path.name}."
            try:
                return _finish_load(default_path, fallback)
            except Exception as fallback_exc:
                print(f"YOLO model {default_path.name} failed to load: {_short_error(fallback_exc)}")
                message = f"{path.name} failed to load. {default_path.name} failed to load: {_short_error(fallback_exc)}"
                return None, None, None, message
        return None, None, None, f"{path.name} failed to load: {_short_error(exc)}"

def _ensure_yolo():
    # Reload when the saved file name or the file's mtime changes. A failed file is not retried
    # until one of those changes, so the next detection stays fast.
    global yolo_model, _yolo_loaded_key, _yolo_attempt_key, _yolo_message
    stored, choice, note = _model_choice()
    path = _model_file(choice)
    mtime = None
    if path is not None:
        try:
            mtime = path.stat().st_mtime_ns
        except OSError:
            path = None
    attempt = (stored, choice, mtime)
    with _yolo_lock:
        if _yolo_attempt_key == attempt:
            return yolo_model, _yolo_message
        if path is not None and yolo_model is not None and _yolo_loaded_key == (str(path), mtime):
            _yolo_attempt_key = attempt
            _yolo_message = note
            return yolo_model, _yolo_message
        loaded, used_path, used_mtime, message = _load_choice(path, note)
        yolo_model = loaded
        if loaded is not None and used_path is not None:
            _yolo_loaded_key = (str(used_path), used_mtime)
        else:
            _yolo_loaded_key = None
        _yolo_attempt_key = attempt
        _yolo_message = message
        if message:
            print(message)
        return yolo_model, _yolo_message

def _image_from_encoded(value):
    if not isinstance(value, str) or not value.strip():
        return None
    text = value.strip()
    if text.startswith("data:"):
        comma = text.find(",")
        if comma < 0:
            return None
        text = text[comma + 1 :]
    try:
        raw = base64.b64decode(text, validate=False)
    except Exception:
        return None
    if not raw:
        return None
    array = np.frombuffer(raw, dtype=np.uint8)
    return cv2.imdecode(array, cv2.IMREAD_COLOR)

def _load_detect_image(req):
    if req is not None and req.image:
        image = _image_from_encoded(req.image)
        if image is None:
            return None, "Could not read the image"
        return image, None
    if cameras._latest_snapshot is None:
        return None, "No image and no snapshot"
    return cameras._latest_snapshot.copy(), None

def _part_ic(names, class_id):
    # Trained class names are part IC codes (2BBMNM08), never kit codes (6PBBK001).
    if isinstance(names, dict):
        ic = names.get(class_id, names.get(str(class_id), class_id))
    else:
        ic = names[class_id]
    return str(ic)

def _roi_slice(image, roi):
    height, width = image.shape[:2]
    if width < 1 or height < 1:
        return image, 0, 0
    x = int(round(float(roi["x"]) * width))
    y = int(round(float(roi["y"]) * height))
    rw = int(round(float(roi["w"]) * width))
    rh = int(round(float(roi["h"]) * height))
    x = max(0, min(x, width - 1))
    y = max(0, min(y, height - 1))
    rw = max(1, min(rw, width - x))
    rh = max(1, min(rh, height - y))
    return image[y:y + rh, x:x + rw], x, y

def _detect_view(image):
    # Posted images and the latest camera snapshot both come through here.
    roi = _clean_detect_roi(load_bom_source().get("detectRoi"))
    if roi is None:
        return image, 0, 0
    return _roi_slice(image, roi)

def _yolo_detections(image, model):
    view, origin_x, origin_y = _detect_view(image)
    result = model.predict(view, verbose=False)[0]
    names = result.names or {}
    boxes = result.boxes
    if boxes is None or len(boxes) == 0:
        return []
    found = []
    for box, conf, cls_id in zip(boxes.xyxy.tolist(), boxes.conf.tolist(), boxes.cls.tolist()):
        x1, y1, x2, y2 = (int(round(value)) for value in box)
        found.append({
            "ic": _part_ic(names, int(cls_id)),
            "confidence": round(float(conf), 4),
            "bbox": [x1 + origin_x, y1 + origin_y, x2 + origin_x, y2 + origin_y],
        })
    return found

def _detect_response(detections, image_base64=None, message=None):
    # image_base64 is the original frame. The page draws boxes over it.
    payload = {
        "detections": detections,
        "image_base64": image_base64,
    }
    if message:
        payload["message"] = message
    return payload

router = APIRouter()

@router.post("/detect")
def detect(req: Optional[DetectRequest] = Body(default=None)):
    # Returns part ICs from the model. The kit code stays with the barcode lookup.
    # An image in the body wins. Otherwise use the latest snapshot.
    # A config change is picked up here when Settings has not already reloaded the file.
    model, model_message = _ensure_yolo()
    if model is None:
        return _detect_response([], message=model_message or "YOLO model is not loaded")

    image, error = _load_detect_image(req)
    if image is None:
        return _detect_response([], message=error or "No image and no snapshot")

    try:
        detections = _yolo_detections(image, model)
    except Exception as exc:
        return _detect_response([], message=f"Detect failed: {exc}")

    encoded = _jpeg_base64(image)
    if not encoded:
        return _detect_response(detections, message="Could not encode the image")
    return _detect_response(detections, encoded)
