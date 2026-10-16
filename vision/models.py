from pathlib import Path

from fastapi import APIRouter
from pydantic import BaseModel, field_validator

from config import (
    DEFAULT_MODEL_NAME,
    MODEL_DIR,
    _clean_model_name,
    load_bom_source,
    save_bom_source,
)

class ModelUpdate(BaseModel):
    # File name only, for example best.pt. An empty string selects the default.
    filename: str = ""

    @field_validator("filename", mode="before")
    @classmethod
    def reject_non_str(cls, value):
        if value is None:
            return ""
        if not isinstance(value, str):
            raise ValueError("Model must be a file name")
        return value

def _list_model_files():
    if not MODEL_DIR.is_dir():
        return []
    root = MODEL_DIR.resolve()
    names = []
    for path in MODEL_DIR.iterdir():
        if not path.is_file() or not _clean_model_name(path.name):
            continue
        try:
            path.resolve().relative_to(root)
        except (OSError, ValueError):
            continue
        names.append(path.name)
    names.sort(key=str.lower)
    return names

def _match_model_file(name, files):
    cleaned = _clean_model_name(name)
    if not cleaned:
        return ""
    for item in files:
        if item.lower() == cleaned.lower():
            return item
    return ""

def _model_file(name):
    cleaned = _clean_model_name(name)
    if not cleaned:
        return None
    root = MODEL_DIR.resolve()
    try:
        resolved = (root / cleaned).resolve()
        resolved.relative_to(root)
    except (OSError, ValueError):
        return None
    if resolved.is_file():
        return resolved
    if not root.is_dir():
        return None
    for path in root.iterdir():
        if not path.is_file() or path.name.lower() != cleaned.lower():
            continue
        try:
            found = path.resolve()
            found.relative_to(root)
        except (OSError, ValueError):
            return None
        return found
    return None

def _model_choice():
    # Returns (stored name, file to load, note). A missing or empty choice uses best.pt.
    files = _list_model_files()
    stored = _clean_model_name(load_bom_source().get("model"))
    match = _match_model_file(stored, files)
    default_name = _match_model_file(DEFAULT_MODEL_NAME, files)
    if match:
        return stored, match, ""
    if default_name:
        if stored:
            return stored, default_name, f"{stored} was not found. Using {default_name}."
        return "", default_name, ""
    if stored:
        return stored, DEFAULT_MODEL_NAME, f"{stored} was not found. {DEFAULT_MODEL_NAME} was not found."
    return "", DEFAULT_MODEL_NAME, f"YOLO model {DEFAULT_MODEL_NAME} was not found."

def _model_payload():
    # detect imports this module. Load it here so the two can call each other.
    import detect

    model, load_message = detect._ensure_yolo()
    files = _list_model_files()
    stored, choice, note = _model_choice()
    selected = _match_model_file(stored, files) or _match_model_file(choice, files)
    active = choice
    with detect._yolo_lock:
        if model is not None and detect._yolo_loaded_key:
            active = Path(detect._yolo_loaded_key[0]).name
    return {
        "model": selected or active,
        "active": active,
        "files": files,
        "loaded": model is not None,
        "message": load_message or note or "",
    }

router = APIRouter()

@router.get("/settings/model")
def get_model():
    return _model_payload()

@router.post("/settings/model")
def update_model(body: ModelUpdate):
    files = _list_model_files()
    raw = body.filename.strip()
    cleaned = _clean_model_name(body.filename)
    if raw and not cleaned:
        return {"status": "fail", "message": "Choose a .pt or .onnx file in vision/models."}
    match = _match_model_file(cleaned, files) if cleaned else ""
    if cleaned and not match:
        return {"status": "fail", "message": f"{cleaned} was not found in vision/models."}
    config = load_bom_source()
    config["model"] = match
    save_bom_source(config)
    payload = _model_payload()
    if payload["loaded"]:
        if payload["message"]:
            text = f"Saved. {payload['message']}"
        else:
            text = f"Saved. Detection uses {payload['active']}."
    else:
        text = f"Saved. {payload['message'] or 'The model is not loaded.'}"
    payload["message"] = text
    payload["status"] = "pass"
    return payload
