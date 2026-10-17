import json
import urllib.error
import urllib.request
from pathlib import Path
from typing import Optional
from urllib.parse import urlparse

from fastapi import APIRouter
from pydantic import BaseModel, ValidationError, field_validator, model_validator

# Local BoM source choice. A missing file means the JSON sample BoM.
# The settings-page unlock password is not stored here.
# A SQL Server Authentication password is stored with the other connection fields.
# config.local.json stores the BoM source, the operator ERP button, the detect ROI, the model file name, and the primary camera.
# An API key is stored with the other API fields. POST /bom/api posts the job card number.
# bom-source.json is read only when config.local.json is absent.
BOM_SOURCE_PATH = Path(__file__).parent / "bom-source.json"
CONFIG_LOCAL_PATH = Path(__file__).parent / "config.local.json"
DEFAULT_SQL_DRIVER = "ODBC Driver 18 for SQL Server"
DEFAULT_SCAN_SECONDS = 8
MIN_SCAN_SECONDS = 1
MAX_SCAN_SECONDS = 60
MODEL_DIR = Path(__file__).parent / "models"
DEFAULT_MODEL_NAME = "best.pt"
_MODEL_SUFFIXES = {".pt", ".onnx"}
MAX_CAMERA_INDEX = 32
CAMERA_RESOLUTIONS = ("640x480", "1280x720", "1920x1080")

class ApiSourceUpdate(BaseModel):
    baseUrl: Optional[str] = ""
    path: Optional[str] = ""
    key: Optional[str] = ""

class BomSourceUpdate(BaseModel):
    source: str
    server: Optional[str] = ""
    database: Optional[str] = ""
    auth: Optional[str] = "windows"
    user: Optional[str] = ""
    password: Optional[str] = ""
    # None keeps the API fields already stored. The settings page sends the object.
    api: Optional[ApiSourceUpdate] = None

class ErpUpdate(BaseModel):
    name: Optional[str] = ""
    url: Optional[str] = ""

class ScanUpdate(BaseModel):
    seconds: int

    @field_validator("seconds", mode="before")
    @classmethod
    def reject_bool_seconds(cls, value):
        # JSON true must not become 1. Pydantic would coerce bool before int().
        if isinstance(value, bool):
            raise ValueError("Scan interval must be a whole number of seconds")
        return value

class TestModeUpdate(BaseModel):
    enabled: bool

    @field_validator("enabled", mode="before")
    @classmethod
    def reject_non_bool(cls, value):
        # JSON 1 or "true" must not turn Test mode on.
        if not isinstance(value, bool):
            raise ValueError("Test mode must be true or false")
        return value

class DetectRoiBox(BaseModel):
    # Fractions of the image, origin at the top left. Not the barcode ROI.
    x: float
    y: float
    w: float
    h: float

    @field_validator("x", "y", "w", "h", mode="before")
    @classmethod
    def reject_non_number(cls, value):
        # JSON true must not become 1.
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError("ROI values must be numbers from 0 to 1")
        return value

    @model_validator(mode="after")
    def inside_image(self):
        if self.w < 0.01 or self.h < 0.01:
            raise ValueError("ROI is too small")
        if (
            self.x < 0
            or self.y < 0
            or self.w > 1
            or self.h > 1
            or self.x + self.w > 1.0001
            or self.y + self.h > 1.0001
        ):
            raise ValueError("ROI must sit inside the image")
        return self

class DetectRoiUpdate(BaseModel):
    roi: Optional[DetectRoiBox] = None

class CameraUpdate(BaseModel):
    # Fields left out of the body stay as they are. Settings sends every field.
    # The operator page sends index only. applyOnly updates the open camera
    # without replacing the saved settings.
    index: Optional[int] = None
    resolution: Optional[str] = None
    focus: Optional[int] = None
    brightness: Optional[int] = None
    contrast: Optional[int] = None
    applyOnly: Optional[bool] = None

    @field_validator("index", "focus", "brightness", "contrast", mode="before")
    @classmethod
    def reject_bool_and_fraction(cls, value):
        # JSON true must not become 1. A fractional value is not a camera step.
        if isinstance(value, bool):
            raise ValueError("Camera values must be whole numbers")
        if isinstance(value, float) and not value.is_integer():
            raise ValueError("Camera values must be whole numbers")
        return value

def _blank_erp():
    return {"name": "", "url": ""}

def _blank_api():
    return {"baseUrl": "", "path": "", "key": ""}

def _blank_bom_source():
    return {
        "source": "json",
        "mssql": {
            "driver": DEFAULT_SQL_DRIVER,
            "server": "",
            "database": "",
            "auth": "windows",
            "user": "",
            "password": "",
        },
        "api": _blank_api(),
        "erp": _blank_erp(),
        "scanSeconds": DEFAULT_SCAN_SECONDS,
        "testMode": False,
        "detectRoi": None,
        "barcodeRoi": None,
        # Empty means best.pt. A saved name is a file in vision/models.
        "model": "",
        "camera": _blank_camera(),
    }

def _read_json_dict(path):
    try:
        with open(path, encoding="utf-8") as f:
            saved = json.load(f)
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(saved, dict):
        return None
    return saved

def _apply_saved_source(config, saved):
    source = saved.get("source")
    if source in ("json", "mssql", "api"):
        config["source"] = source
    mssql = saved.get("mssql")
    if isinstance(mssql, dict):
        for key in ("server", "database", "user", "password"):
            value = mssql.get(key)
            if isinstance(value, str):
                config["mssql"][key] = value
        driver = mssql.get("driver")
        if isinstance(driver, str) and driver.strip():
            config["mssql"]["driver"] = driver.strip()
        if mssql.get("auth") in ("windows", "sql"):
            config["mssql"]["auth"] = mssql["auth"]
    # Windows Authentication does not use a SQL login.
    if config["mssql"]["auth"] == "windows":
        config["mssql"]["user"] = ""
        config["mssql"]["password"] = ""
    return config

def _apply_saved_api(config, saved):
    api = saved.get("api")
    if not isinstance(api, dict):
        return config
    for field in ("baseUrl", "path", "key"):
        value = api.get(field)
        if isinstance(value, str):
            config["api"][field] = value.strip()
    return config

def _clean_api(base_url, path, key, check_url):
    base_url = (base_url or "").strip()
    path = (path or "").strip()
    key = (key or "").strip()
    if len(base_url) > 2000:
        return None, "Base URL is too long"
    if len(path) > 2000:
        return None, "Endpoint path is too long"
    if len(key) > 2000:
        return None, "API key is too long"
    if check_url and base_url:
        parsed = urlparse(base_url)
        if parsed.scheme not in ("http", "https") or not parsed.netloc:
            return None, "Base URL must start with http:// or https://"
    return {"baseUrl": base_url, "path": path, "key": key}, None

def _apply_saved_erp(config, saved):
    erp = saved.get("erp")
    if not isinstance(erp, dict):
        return config
    name = erp.get("name")
    url = erp.get("url")
    if isinstance(name, str):
        config["erp"]["name"] = name.strip()
    if isinstance(url, str):
        config["erp"]["url"] = url.strip()
    return config

def _clean_scan_seconds(raw):
    # bool is an int in Python. A JSON true must not become 1 second.
    if isinstance(raw, bool):
        return None
    if isinstance(raw, str):
        raw = raw.strip()
        if not raw:
            return None
    if isinstance(raw, float) and not raw.is_integer():
        return None
    try:
        seconds = int(raw)
    except (TypeError, ValueError):
        return None
    if seconds < MIN_SCAN_SECONDS or seconds > MAX_SCAN_SECONDS:
        return None
    return seconds

def _apply_saved_scan(config, saved):
    seconds = _clean_scan_seconds(saved.get("scanSeconds"))
    if seconds is not None:
        config["scanSeconds"] = seconds
    return config

def _apply_saved_test_mode(config, saved):
    mode = saved.get("testMode")
    if isinstance(mode, bool):
        config["testMode"] = mode
    return config

def _clean_detect_roi(raw):
    if raw is None:
        return None
    try:
        box = DetectRoiBox.model_validate(raw)
    except (ValidationError, TypeError):
        return None
    width = min(box.w, 1 - box.x)
    height = min(box.h, 1 - box.y)
    return {
        "x": round(box.x, 4),
        "y": round(box.y, 4),
        "w": round(width, 4),
        "h": round(height, 4),
    }

def _apply_saved_detect_roi(config, saved):
    if "detectRoi" in saved:
        config["detectRoi"] = _clean_detect_roi(saved.get("detectRoi"))
    return config

def _clean_barcode_roi(raw):
    # Kept beside detectRoi. The operator crops the barcode frame. Detection does not.
    if not isinstance(raw, dict):
        return None
    cleaned = {}
    for key in ("x", "y", "width", "height"):
        value = raw.get(key)
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return None
        if value != value or value in (float("inf"), float("-inf")):
            return None
        cleaned[key] = value
    return cleaned

def _apply_saved_barcode_roi(config, saved):
    if "barcodeRoi" in saved:
        config["barcodeRoi"] = _clean_barcode_roi(saved.get("barcodeRoi"))
    return config

def _clean_model_name(raw):
    # A config value is a file name, never a path. Reject separators and other suffixes.
    if not isinstance(raw, str):
        return ""
    name = raw.strip()
    if not name or len(name) > 180:
        return ""
    if name != Path(name).name:
        return ""
    if "/" in name or "\\" in name or ".." in name or "\x00" in name:
        return ""
    if Path(name).suffix.lower() not in _MODEL_SUFFIXES:
        return ""
    return name

def _apply_saved_model(config, saved):
    if "model" in saved:
        config["model"] = _clean_model_name(saved.get("model"))
    return config

def _blank_camera():
    return {
        "index": 0,
        "resolution": "",
        "focus": None,
        "brightness": None,
        "contrast": None,
    }

def _clean_camera_index(raw):
    # bool is an int in Python. JSON true must not become camera 1.
    if isinstance(raw, bool):
        return None
    if isinstance(raw, float) and not raw.is_integer():
        return None
    try:
        index = int(raw)
    except (TypeError, ValueError):
        return None
    if index < 0 or index > MAX_CAMERA_INDEX:
        return None
    return index

def _clean_resolution(raw):
    if not isinstance(raw, str):
        return ""
    value = raw.strip().lower().replace("×", "x").replace(" ", "")
    if value in CAMERA_RESOLUTIONS:
        return value
    return ""

def _clean_camera_level(raw):
    """A stored focus, brightness, or contrast step, or None when it is unset or invalid."""
    if raw is None or isinstance(raw, bool):
        return None
    if isinstance(raw, float) and not raw.is_integer():
        return None
    try:
        level = int(raw)
    except (TypeError, ValueError):
        return None
    if level < 0 or level > 100:
        return None
    return level

def _stored_camera(raw):
    camera = _blank_camera()
    if not isinstance(raw, dict):
        return camera
    index = _clean_camera_index(raw.get("index"))
    if index is not None:
        camera["index"] = index
    camera["resolution"] = _clean_resolution(raw.get("resolution"))
    for field in ("focus", "brightness", "contrast"):
        camera[field] = _clean_camera_level(raw.get(field))
    return camera

def _apply_saved_camera(config, saved):
    if "camera" in saved:
        config["camera"] = _stored_camera(saved.get("camera"))
    return config

def _camera_level_or_error(raw, label):
    if raw is None:
        return None, None
    level = _clean_camera_level(raw)
    if level is None:
        return None, f"{label} must be a whole number from 0 to 100, or blank"
    return level, None

def _resolution_or_error(raw):
    if raw is None or (isinstance(raw, str) and not raw.strip()):
        return "", None
    cleaned = _clean_resolution(raw)
    if not cleaned:
        return None, "Resolution must be 640x480, 1280x720, 1920x1080, or blank"
    return cleaned, None

def _clean_erp(name, url):
    name = " ".join((name or "").split())
    url = (url or "").strip()
    if len(name) > 40:
        return None, "Button name must be 40 characters or fewer"
    if len(url) > 2000:
        return None, "URL is too long"
    if not url:
        if name:
            return None, "URL is required"
        return _blank_erp(), None
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        return None, "URL must start with http:// or https://"
    if not name:
        name = "ERP"
    return {"name": name, "url": url}, None

def _frame_ancestors_allow(csp):
    """Return True, False, or None when the header has no frame-ancestors."""
    for directive in (csp or "").split(";"):
        parts = directive.strip().split()
        if not parts or parts[0].lower() != "frame-ancestors":
            continue
        sources = [item.lower() for item in parts[1:]]
        if not sources or "'none'" in sources:
            return False
        if "*" in sources:
            return True
        # 'self' is the ERP origin. The vision page is the parent, so that
        # does not make the iframe allowed.
        for source in sources:
            if "127.0.0.1" in source or "localhost" in source or "tauri.localhost" in source:
                return True
        return False
    return None

def _headers_allow_frame(headers):
    raw = headers.get("X-Frame-Options") if headers is not None else None
    if raw:
        directive = raw.split(",")[0].strip().lower()
        if directive in ("deny", "sameorigin"):
            return False
    values = []
    if headers is not None and hasattr(headers, "get_all"):
        values = headers.get_all("Content-Security-Policy") or []
    elif headers is not None:
        single = headers.get("Content-Security-Policy")
        if single:
            values = [single]
    for csp in values:
        allowed = _frame_ancestors_allow(csp)
        if allowed is False:
            return False
    return True

def _erp_can_frame(url):
    if not url:
        return False
    request = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    response = None
    try:
        response = urllib.request.urlopen(request, timeout=8)
    except urllib.error.HTTPError as exc:
        response = exc
    except Exception:
        return False
    try:
        return _headers_allow_frame(response.headers)
    finally:
        close = getattr(response, "close", None)
        if close is not None:
            close()

def load_bom_source():
    config = _blank_bom_source()
    if CONFIG_LOCAL_PATH.exists():
        saved = _read_json_dict(CONFIG_LOCAL_PATH)
        if saved is None:
            return config
        _apply_saved_source(config, saved)
        _apply_saved_api(config, saved)
        _apply_saved_erp(config, saved)
        _apply_saved_scan(config, saved)
        _apply_saved_test_mode(config, saved)
        _apply_saved_detect_roi(config, saved)
        _apply_saved_barcode_roi(config, saved)
        _apply_saved_model(config, saved)
        return _apply_saved_camera(config, saved)
    if BOM_SOURCE_PATH.exists():
        saved = _read_json_dict(BOM_SOURCE_PATH)
        if saved is None:
            return config
        _apply_saved_source(config, saved)
        _apply_saved_api(config, saved)
        _apply_saved_erp(config, saved)
        _apply_saved_scan(config, saved)
        _apply_saved_test_mode(config, saved)
        _apply_saved_detect_roi(config, saved)
        _apply_saved_barcode_roi(config, saved)
        _apply_saved_model(config, saved)
        return _apply_saved_camera(config, saved)
    return config

def save_bom_source(config):
    erp = config.get("erp") or _blank_erp()
    stored = {
        "source": config["source"],
        "mssql": {
            "driver": config["mssql"].get("driver") or DEFAULT_SQL_DRIVER,
            "server": config["mssql"]["server"],
            "database": config["mssql"]["database"],
            "auth": config["mssql"]["auth"],
            "user": config["mssql"]["user"],
            "password": config["mssql"]["password"],
        },
        "api": {
            "baseUrl": (config.get("api") or {}).get("baseUrl") or "",
            "path": (config.get("api") or {}).get("path") or "",
            "key": (config.get("api") or {}).get("key") or "",
        },
        "erp": {
            "name": erp.get("name") or "",
            "url": erp.get("url") or "",
        },
        "scanSeconds": _clean_scan_seconds(config.get("scanSeconds")) or DEFAULT_SCAN_SECONDS,
        "testMode": config.get("testMode") is True,
        "detectRoi": _clean_detect_roi(config.get("detectRoi")),
        "barcodeRoi": _clean_barcode_roi(config.get("barcodeRoi")),
        "model": _clean_model_name(config.get("model")),
        "camera": _stored_camera(config.get("camera")),
    }
    CONFIG_LOCAL_PATH.write_text(
        json.dumps(stored, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return stored

router = APIRouter()

@router.get("/settings/bom-source")
def get_bom_source():
    return load_bom_source()

@router.post("/settings/bom-source")
def update_bom_source(body: BomSourceUpdate):
    if body.source not in ("json", "mssql", "api"):
        return {"status": "fail", "message": "BoM source must be json, mssql, or api"}
    if body.auth not in ("windows", "sql"):
        return {"status": "fail", "message": "Authentication must be windows or sql"}
    user = (body.user or "").strip()
    password = body.password or ""
    if body.auth == "windows":
        user = ""
        password = ""
    elif not user or not password.strip():
        return {
            "status": "fail",
            "message": "Username and password are required for SQL Server Authentication",
        }
    existing = load_bom_source()
    if body.api is None:
        api = existing.get("api") or _blank_api()
    else:
        api, error = _clean_api(
            body.api.baseUrl,
            body.api.path,
            body.api.key,
            body.source == "api",
        )
        if error:
            return {"status": "fail", "message": error}
    config = _blank_bom_source()
    config["source"] = body.source
    config["mssql"]["driver"] = existing["mssql"].get("driver") or DEFAULT_SQL_DRIVER
    config["mssql"]["server"] = (body.server or "").strip()
    config["mssql"]["database"] = (body.database or "").strip()
    config["mssql"]["auth"] = body.auth
    config["mssql"]["user"] = user
    config["mssql"]["password"] = password
    config["api"] = api
    config["erp"] = existing.get("erp") or _blank_erp()
    config["scanSeconds"] = existing.get("scanSeconds", DEFAULT_SCAN_SECONDS)
    config["testMode"] = existing.get("testMode") is True
    config["detectRoi"] = _clean_detect_roi(existing.get("detectRoi"))
    config["barcodeRoi"] = _clean_barcode_roi(existing.get("barcodeRoi"))
    config["model"] = _clean_model_name(existing.get("model"))
    config["camera"] = _stored_camera(existing.get("camera"))
    saved = save_bom_source(config)
    if body.source == "mssql":
        message = "Saved. Inspect queries MSSQL using vision/bom_queries.sql."
    elif body.source == "api":
        message = "Saved. Look up posts the job card number to the API."
    else:
        message = "Saved. Inspect uses vision/sample-bom.json."
    return {"status": "pass", "message": message, **saved}

@router.get("/settings/erp")
def get_erp():
    erp = load_bom_source().get("erp") or _blank_erp()
    return {"name": erp.get("name") or "", "url": erp.get("url") or ""}

@router.get("/settings/erp/frame")
def erp_frame():
    erp = load_bom_source().get("erp") or _blank_erp()
    url = erp.get("url") or ""
    return {
        "name": erp.get("name") or "",
        "url": url,
        "frame": _erp_can_frame(url) if url else False,
    }

@router.post("/settings/erp")
def update_erp(body: ErpUpdate):
    cleaned, error = _clean_erp(body.name, body.url)
    if error:
        return {"status": "fail", "message": error}
    config = load_bom_source()
    config["erp"] = cleaned
    saved = save_bom_source(config)
    if cleaned["url"]:
        message = "Saved. The operator page shows this ERP button."
    else:
        message = "Saved. The ERP button stays hidden until a URL is set."
    return {
        "status": "pass",
        "message": message,
        "name": saved["erp"]["name"],
        "url": saved["erp"]["url"],
    }

@router.get("/settings/scan")
def get_scan():
    seconds = _clean_scan_seconds(load_bom_source().get("scanSeconds"))
    return {"seconds": seconds or DEFAULT_SCAN_SECONDS}

@router.post("/settings/scan")
def update_scan(body: ScanUpdate):
    seconds = _clean_scan_seconds(body.seconds)
    if seconds is None:
        return {
            "status": "fail",
            "message": f"Scan interval must be a whole number of seconds from {MIN_SCAN_SECONDS} to {MAX_SCAN_SECONDS}",
        }
    config = load_bom_source()
    config["scanSeconds"] = seconds
    saved = save_bom_source(config)
    return {
        "status": "pass",
        "message": f"Saved. The operator page checks for a barcode every {seconds} seconds.",
        "seconds": saved["scanSeconds"],
    }

@router.get("/settings/test-mode")
def get_test_mode():
    return {"enabled": load_bom_source().get("testMode") is True}

@router.post("/settings/test-mode")
def update_test_mode(body: TestModeUpdate):
    config = load_bom_source()
    config["testMode"] = body.enabled
    saved = save_bom_source(config)
    if saved["testMode"]:
        message = "Saved. Enter a kit IC to choose an image for detection."
    else:
        message = "Saved. The operator page uses the camera and barcode flow."
    return {"status": "pass", "message": message, "enabled": saved["testMode"]}

@router.get("/settings/detect-roi")
def get_detect_roi():
    return {"roi": _clean_detect_roi(load_bom_source().get("detectRoi"))}

@router.post("/settings/detect-roi")
def update_detect_roi(body: DetectRoiUpdate):
    config = load_bom_source()
    config["detectRoi"] = _clean_detect_roi(None if body.roi is None else body.roi.model_dump())
    saved = save_bom_source(config)
    if saved["detectRoi"]:
        message = "Saved. Detection uses this region."
    else:
        message = "Saved. Detection uses the full image."
    return {"status": "pass", "message": message, "roi": saved["detectRoi"]}

def _camera_response(saved, message):
    camera = saved.get("camera") or _blank_camera()
    return {
        "status": "pass",
        "message": message,
        "index": camera["index"],
        "resolution": camera["resolution"],
        "focus": camera["focus"],
        "brightness": camera["brightness"],
        "contrast": camera["contrast"],
    }

@router.get("/settings/camera")
def get_camera():
    camera = load_bom_source().get("camera") or _blank_camera()
    return {
        "index": camera["index"],
        "resolution": camera["resolution"],
        "focus": camera["focus"],
        "brightness": camera["brightness"],
        "contrast": camera["contrast"],
    }

@router.post("/settings/camera")
def update_camera(body: CameraUpdate):
    config = load_bom_source()
    camera = _stored_camera(config.get("camera"))
    sent = body.model_fields_set
    if "index" in sent:
        index = _clean_camera_index(body.index)
        if index is None:
            return {
                "status": "fail",
                "message": f"Camera index must be a whole number from 0 to {MAX_CAMERA_INDEX}",
            }
        camera["index"] = index
    if "resolution" in sent:
        resolution, error = _resolution_or_error(body.resolution)
        if error:
            return {"status": "fail", "message": error}
        camera["resolution"] = resolution
    for field, label in (
        ("focus", "Focus"),
        ("brightness", "Brightness"),
        ("contrast", "Contrast"),
    ):
        if field not in sent:
            continue
        level, error = _camera_level_or_error(getattr(body, field), label)
        if error:
            return {"status": "fail", "message": error}
        camera[field] = level
    # Imported here so config can load without the camera stack.
    from cameras import publish_camera_controls

    if body.applyOnly is True:
        publish_camera_controls(camera)
        return _camera_response({"camera": camera}, "Applied.")
    config["camera"] = camera
    saved = save_bom_source(config)
    publish_camera_controls(saved.get("camera") or {})
    return _camera_response(saved, "Saved. The camera uses these image settings.")
