import base64
from contextlib import asynccontextmanager
from typing import Optional

import cv2
import numpy as np
from fastapi import Body, FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, ValidationError, field_validator, model_validator
import uvicorn
import json
import re
import urllib.error
import urllib.request
from decimal import Decimal
from pathlib import Path
from urllib.parse import urlparse

# pyzbar's bundled libzbar needs MSVCR120.dll (VC++ 2013 x64). If that runtime
# is missing, keep /health and /inspect up and report the error from decode routes.
try:
    from pyzbar import pyzbar
except OSError as exc:
    pyzbar = None
    _PYZBAR_ERROR = str(exc)
else:
    _PYZBAR_ERROR = None

@asynccontextmanager
async def _lifespan(_app):
    # Barcode and BoM stay up even when the weights fail to load.
    _load_yolo()
    yield

app = FastAPI(title="LS-Vision Sidecar", lifespan=_lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

BOM_PATH = Path(__file__).parent / "sample-bom.json"
# Local BoM source choice. A missing file means the JSON sample BoM.
# The settings-page unlock password is not stored here.
# A SQL Server Authentication password is stored with the other connection fields.
# config.local.json stores the BoM source, the operator ERP button, and the detect ROI.
# bom-source.json is read only when config.local.json is absent.
BOM_SOURCE_PATH = Path(__file__).parent / "bom-source.json"
CONFIG_LOCAL_PATH = Path(__file__).parent / "config.local.json"
BOM_QUERIES_PATH = Path(__file__).parent / "bom_queries.sql"
BOM_QUERIES_EXAMPLE_PATH = Path(__file__).parent / "bom_queries.example.sql"
DEFAULT_SQL_DRIVER = "ODBC Driver 18 for SQL Server"
DEFAULT_SCAN_SECONDS = 8
MIN_SCAN_SECONDS = 1
MAX_SCAN_SECONDS = 60
MODEL_PATH = Path(__file__).parent / "models" / "best.pt"

# Fixed square on the bench. Change at runtime with POST /barcode/roi.
CAMERA_INDEX = 0
BARCODE_ROI = {"x": 0, "y": 0, "w": 640, "h": 480}

# One open capture shared by the stream and the decode calls.
camera = None
_camera_index = None
# Last raw frame a snapshot held. POST /detect uses it when
# the request does not include an image.
_latest_snapshot = None
yolo_model = None

class InspectRequest(BaseModel):
    barcode: str

class DetectRequest(BaseModel):
    # data-url or raw base64. Omit it to use the latest snapshot.
    image: Optional[str] = None

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

class BomSourceUpdate(BaseModel):
    source: str
    server: Optional[str] = ""
    database: Optional[str] = ""
    auth: Optional[str] = "windows"
    user: Optional[str] = ""
    password: Optional[str] = ""

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

def _blank_erp():
    return {"name": "", "url": ""}

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
        "erp": _blank_erp(),
        "scanSeconds": DEFAULT_SCAN_SECONDS,
        "testMode": False,
        "detectRoi": None,
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
    if source in ("json", "mssql"):
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
        _apply_saved_erp(config, saved)
        _apply_saved_scan(config, saved)
        _apply_saved_test_mode(config, saved)
        return _apply_saved_detect_roi(config, saved)
    if BOM_SOURCE_PATH.exists():
        saved = _read_json_dict(BOM_SOURCE_PATH)
        if saved is None:
            return config
        _apply_saved_source(config, saved)
        _apply_saved_erp(config, saved)
        _apply_saved_scan(config, saved)
        _apply_saved_test_mode(config, saved)
        return _apply_saved_detect_roi(config, saved)
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
        "erp": {
            "name": erp.get("name") or "",
            "url": erp.get("url") or "",
        },
        "scanSeconds": _clean_scan_seconds(config.get("scanSeconds")) or DEFAULT_SCAN_SECONDS,
        "testMode": config.get("testMode") is True,
        "detectRoi": _clean_detect_roi(config.get("detectRoi")),
    }
    CONFIG_LOCAL_PATH.write_text(
        json.dumps(stored, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return stored

# A line that is only GO, "-- ---", or "-- split" divides the two BoM queries.
_QUERY_SPLIT = re.compile(r"^(?:go|--\s*(?:-{3,}|={3,}|split))\s*$", re.IGNORECASE)

def _has_select(sql):
    return re.search(r"\bselect\b", sql, re.IGNORECASE) is not None

def _starts_with_select(sql):
    for line in sql.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("--"):
            continue
        return re.match(r"select\b", stripped, re.IGNORECASE) is not None
    return False

def split_bom_queries(text):
    """Return (kit header SQL, parts SQL), split on a marker or a blank line."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")

    def index_at(marker):
        for index, line in enumerate(lines):
            stripped = line.strip()
            is_split = _QUERY_SPLIT.match(stripped) is not None if marker else stripped == ""
            if not is_split:
                continue
            header = "\n".join(lines[:index])
            parts = "\n".join(lines[index + 1 :])
            # The parts query has to start a new SELECT, so a blank line inside
            # the header statement is not the split.
            if _has_select(header) and _starts_with_select(parts):
                return index
        return None

    split_at = index_at(True)
    if split_at is None:
        split_at = index_at(False)
    if split_at is None:
        return None
    header = "\n".join(lines[:split_at]).strip()
    parts = "\n".join(lines[split_at + 1 :]).strip()
    return header, parts

def load_bom_queries():
    if BOM_QUERIES_PATH.exists():
        path = BOM_QUERIES_PATH
    elif BOM_QUERIES_EXAMPLE_PATH.exists():
        path = BOM_QUERIES_EXAMPLE_PATH
    else:
        raise FileNotFoundError(
            "BoM queries were not found. Add vision/bom_queries.sql or vision/bom_queries.example.sql."
        )
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise FileNotFoundError(f"Could not read {path.name}.") from exc
    split = split_bom_queries(text)
    if split is None:
        raise ValueError(
            "BoM query file must contain a kit header query and a parts query, separated by a blank line."
        )
    return split

def _odbc_brace(value):
    return "{" + value.replace("}", "}}") + "}"

def _odbc_pair(key, value):
    if any(character in value for character in ";{}="):
        return f"{key}={_odbc_brace(value)}"
    return f"{key}={value}"

def mssql_connection_string(mssql):
    server = (mssql.get("server") or "").strip()
    database = (mssql.get("database") or "").strip()
    if not server or not database:
        raise ValueError(
            "MSSQL server and database are required. Save them on the Settings page or in vision/config.local.json."
        )
    auth = mssql.get("auth") or "windows"
    user = (mssql.get("user") or "").strip()
    password = mssql.get("password") or ""
    if auth == "sql" and (not user or not str(password).strip()):
        raise ValueError("Username and password are required for SQL Server Authentication.")
    driver = (mssql.get("driver") or DEFAULT_SQL_DRIVER).strip() or DEFAULT_SQL_DRIVER
    parts = [
        f"DRIVER={_odbc_brace(driver)}",
        _odbc_pair("SERVER", server),
        _odbc_pair("DATABASE", database),
    ]
    # Driver 18 encrypts by default and rejects an untrusted certificate unless
    # Encrypt is set explicitly. optional matches a server that is not forcing TLS.
    parts.append("Encrypt=optional")
    parts.append("TrustServerCertificate=yes")
    if auth == "sql":
        parts.append(_odbc_pair("UID", user))
        parts.append(_odbc_pair("PWD", password))
    else:
        parts.append("Trusted_Connection=yes")
    return ";".join(parts) + ";"

def _scrub_secret(text, secret):
    if secret:
        return text.replace(secret, "********")
    return text

def _connect_mssql(mssql):
    connection_string = mssql_connection_string(mssql)
    try:
        import pyodbc
    except ImportError as exc:
        raise RuntimeError("pyodbc is not installed. SQL Server lookup is unavailable.") from exc
    # Set before the handshake. The string keyword is ignored when Encrypt is Strict.
    # 1223 is SQL_COPT_SS_ENCRYPT (0 = off/optional). 1228 trusts the server certificate.
    try:
        connection = pyodbc.connect(
            connection_string,
            timeout=5,
            attrs_before={1223: 0, 1228: 1},
        )
    except Exception as exc:
        message = _scrub_secret(str(exc), mssql.get("password") or "")
        raise RuntimeError(f"Could not query MSSQL: {message}") from None
    connection.autocommit = True
    return connection

def _bind_kit_ic(sql):
    return re.sub(r"@kitIC\b", "?", sql, flags=re.IGNORECASE)

def _query_rows(cursor, sql, kit_ic):
    bound = _bind_kit_ic(sql)
    cursor.execute(bound, (kit_ic,) * bound.count("?"))
    if cursor.description is None:
        return []
    columns = [column[0] for column in cursor.description]
    return [dict(zip(columns, row)) for row in cursor.fetchall()]

def _row_value(row, *names):
    lookup = {str(key).lower(): value for key, value in row.items()}
    for name in names:
        if name in lookup:
            return lookup[name]
    return None

def _as_number(value):
    if value is None or value == "":
        return None
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        return int(value) if value.is_integer() else value
    if isinstance(value, Decimal):
        if value == value.to_integral_value():
            return int(value)
        return float(value)
    number = float(str(value).strip())
    return int(number) if number.is_integer() else number

def _as_text(value):
    if value is None:
        return ""
    return str(value).strip()

def _job_from_rows(header_rows, part_rows):
    header = header_rows[0]
    expected = _row_value(header, "expectedkitcount", "expected_kit_count", "labelcount")
    parts = []
    for row in part_rows:
        ic = _as_text(_row_value(row, "ic", "cc"))
        if not ic:
            continue
        qty = _as_number(_row_value(row, "qty", "quantity"))
        parts.append(
            {
                "ic": ic,
                "des": _as_text(_row_value(row, "des", "description")),
                "qty": 0 if qty is None else qty,
            }
        )
    return {
        "des": _as_text(_row_value(header, "des", "kitname", "kit_name", "description")),
        "expectedKitCount": None if expected is None else _as_number(expected),
        "parts": parts,
    }

def _inspect_fail(barcode, message):
    return {
        "barcode": barcode,
        "status": "fail",
        "message": message,
        "kits": [],
    }

def _inspect_pass(barcode, job):
    kits_present = 2  # placeholder until real detection; change as needed for testing
    return {
        "barcode": barcode,
        "status": "pass",
        "message": f"BoM loaded – {kits_present} kit(s) on bench",
        "kitCode": barcode,
        "des": job.get("des"),
        "expectedKitCount": job.get("expectedKitCount"),
        "parts": job.get("parts", []),
        "kitsFound": kits_present,
        "kits": [
            {"id": i + 1, "status": "pass", "missing": [], "extra": []}
            for i in range(kits_present)
        ],
    }

def _mssql_error_message(exc, source):
    password = ""
    mssql = source.get("mssql") if isinstance(source, dict) else None
    if isinstance(mssql, dict):
        password = mssql.get("password") or ""
    text = _scrub_secret(str(exc).strip() or exc.__class__.__name__, password)
    if isinstance(exc, (FileNotFoundError, ValueError, RuntimeError)):
        return text
    return f"Could not query MSSQL: {text}"

def _inspect_mssql(barcode, source):
    header_sql, parts_sql = load_bom_queries()
    connection = _connect_mssql(source["mssql"])
    try:
        cursor = connection.cursor()
        header_rows = _query_rows(cursor, header_sql, barcode)
        if not header_rows:
            return _inspect_fail(barcode, "Barcode not found in BoM")
        part_rows = _query_rows(cursor, parts_sql, barcode)
    finally:
        connection.close()
    return _inspect_pass(barcode, _job_from_rows(header_rows, part_rows))

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

def _clean_code(text):
    # Trailing "/" was a temporary Code 39 suffix. Revisit when labels move to QR.
    if text.endswith("/"):
        return text[:-1]
    return text

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
            "data": _clean_code(text),
            "type": barcode.type,
        })
    return codes

def _pyzbar_load_error():
    return (
        "pyzbar failed to load libzbar. Install the Visual C++ 2013 x64 redistributable (MSVCR120.dll). "
        + (_PYZBAR_ERROR or "")
    )

def _jpeg_base64(image):
    ok, buffer = cv2.imencode(".jpg", image, [int(cv2.IMWRITE_JPEG_QUALITY), 80])
    if not ok:
        return None
    return base64.b64encode(buffer.tobytes()).decode("ascii")

def _jpeg_data_url(image):
    encoded = _jpeg_base64(image)
    if not encoded:
        return None
    return f"data:image/jpeg;base64,{encoded}"

def _remember_snapshot(image):
    global _latest_snapshot
    if image is None:
        return
    _latest_snapshot = image.copy()

def _grab_frame(index):
    cap = get_camera(index)
    ret, frame = cap.read()
    if not ret or frame is None:
        return None
    return frame

def _codes_from_frame(frame):
    decoded = pyzbar.decode(frame)
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
                "x": obj.rect.left,
                "y": obj.rect.top,
                "w": obj.rect.width,
                "h": obj.rect.height,
            },
        })
    return results

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
    frame = _grab_frame(index)
    if frame is None:
        return {"status": "fail", "message": "No frame"}

    if pyzbar is None:
        return {
            "status": "fail",
            "message": _pyzbar_load_error(),
            "codes": [],
        }

    results = _codes_from_frame(frame)
    return {
        "status": "pass" if results else "fail",
        "message": f"Found {len(results)} code(s)" if results else "No code detected",
        "codes": results
    }

@app.get("/camera/snapshot")
def camera_snapshot(index: int = 0):
    # Freeze this frame only when its barcode is in the current BoM (JSON or MSSQL).
    frame = _grab_frame(index)
    if frame is None:
        return {"status": "fail", "snapshot": False, "message": "No frame"}

    if pyzbar is None:
        return {
            "status": "fail",
            "snapshot": False,
            "message": _pyzbar_load_error(),
            "codes": [],
        }

    codes = _codes_from_frame(frame)
    if not codes:
        return {
            "status": "idle",
            "snapshot": False,
            "message": "No code detected",
            "codes": [],
        }

    barcode = codes[0]["data"]
    looked = lookup_bom(barcode)
    if looked.get("status") != "pass":
        return {
            "status": "fail",
            "snapshot": False,
            "barcode": barcode,
            "message": looked.get("message") or "Barcode not found in BoM",
            "codes": codes,
        }

    image = _jpeg_data_url(frame)
    if not image:
        return {
            "status": "fail",
            "snapshot": False,
            "barcode": barcode,
            "message": "Could not encode the frame",
        }

    _remember_snapshot(frame)
    return {
        **looked,
        "snapshot": True,
        "image": image,
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

def _load_yolo():
    global yolo_model
    yolo_model = None
    if not MODEL_PATH.is_file():
        print(f"YOLO model not found at {MODEL_PATH}")
        return
    try:
        from ultralytics import YOLO
        loaded = YOLO(str(MODEL_PATH))
        # First predict pays the device setup cost. Do it before the operator clicks.
        loaded.predict(np.zeros((64, 64, 3), dtype=np.uint8), verbose=False)
        yolo_model = loaded
        names = getattr(loaded, "names", {})
        labels = list(names.values()) if isinstance(names, dict) else list(names)
        print(f"Loaded YOLO model {MODEL_PATH.name} with {len(labels)} part classes")
    except Exception as exc:
        yolo_model = None
        print(f"YOLO model failed to load: {exc}")

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
    if _latest_snapshot is None:
        return None, "No image and no snapshot"
    return _latest_snapshot.copy(), None

def _color_for_ic(ic):
    palette = (
        (0, 180, 60),
        (0, 165, 255),
        (255, 140, 0),
        (60, 60, 230),
        (200, 60, 200),
        (0, 200, 200),
        (180, 180, 0),
        (0, 100, 255),
    )
    total = sum(ic.encode("utf-8"))
    return palette[total % len(palette)]

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

def _yolo_detections(image):
    view, origin_x, origin_y = _detect_view(image)
    result = yolo_model.predict(view, verbose=False)[0]
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

def _annotate_detections(image, detections):
    annotated = image.copy()
    height, width = annotated.shape[:2]
    scale = max(height, width) / 1600.0
    thickness = max(2, int(round(scale * 2)))
    font = cv2.FONT_HERSHEY_SIMPLEX
    font_scale = max(0.4, min(0.75, scale * 0.45))
    text_thickness = max(1, thickness - 1)
    ordered = sorted(detections, key=lambda item: item["confidence"])
    for item in ordered:
        x1, y1, x2, y2 = item["bbox"]
        x1 = max(0, min(width - 1, int(x1)))
        y1 = max(0, min(height - 1, int(y1)))
        x2 = max(0, min(width - 1, int(x2)))
        y2 = max(0, min(height - 1, int(y2)))
        if x2 <= x1 or y2 <= y1:
            continue
        color = _color_for_ic(item["ic"])
        cv2.rectangle(annotated, (x1, y1), (x2, y2), color, thickness)
        label = item["ic"]
        (text_width, text_height), baseline = cv2.getTextSize(
            label, font, font_scale, text_thickness
        )
        chip_top = y1 - text_height - baseline - 6
        if chip_top < 0:
            chip_top = min(y1, height - text_height - baseline - 6)
        chip_top = max(0, chip_top)
        chip_bottom = min(height - 1, chip_top + text_height + baseline + 6)
        chip_right = min(width - 1, x1 + text_width + 8)
        cv2.rectangle(annotated, (x1, chip_top), (chip_right, chip_bottom), color, -1)
        cv2.putText(
            annotated,
            label,
            (x1 + 4, max(text_height, chip_bottom - baseline - 2)),
            font,
            font_scale,
            (255, 255, 255),
            text_thickness,
            cv2.LINE_AA,
        )
    return annotated

def _detect_response(detections, image=None, message=None):
    payload = {
        "detections": detections,
        "annotated_image_base64": _jpeg_base64(image) if image is not None else None,
    }
    if message:
        payload["message"] = message
    return payload

@app.post("/detect")
def detect(req: Optional[DetectRequest] = Body(default=None)):
    # Returns part ICs from the model. The kit code stays with the barcode lookup.
    # An image in the body wins. Otherwise use the latest snapshot.
    if yolo_model is None:
        return _detect_response([], message="YOLO model is not loaded")

    image, error = _load_detect_image(req)
    if image is None:
        return _detect_response([], message=error or "No image and no snapshot")

    try:
        detections = _yolo_detections(image)
        annotated = _annotate_detections(image, detections)
    except Exception as exc:
        return _detect_response([], message=f"Detect failed: {exc}")

    encoded = _jpeg_base64(annotated)
    if not encoded:
        return _detect_response(detections, message="Could not encode the image")
    return {
        "detections": detections,
        "annotated_image_base64": encoded,
    }

@app.get("/settings/bom-source")
def get_bom_source():
    return load_bom_source()

@app.post("/settings/bom-source")
def update_bom_source(body: BomSourceUpdate):
    if body.source not in ("json", "mssql"):
        return {"status": "fail", "message": "BoM source must be json or mssql"}
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
    config = _blank_bom_source()
    config["source"] = body.source
    config["mssql"]["driver"] = existing["mssql"].get("driver") or DEFAULT_SQL_DRIVER
    config["mssql"]["server"] = (body.server or "").strip()
    config["mssql"]["database"] = (body.database or "").strip()
    config["mssql"]["auth"] = body.auth
    config["mssql"]["user"] = user
    config["mssql"]["password"] = password
    config["erp"] = existing.get("erp") or _blank_erp()
    config["scanSeconds"] = existing.get("scanSeconds", DEFAULT_SCAN_SECONDS)
    config["testMode"] = existing.get("testMode") is True
    config["detectRoi"] = _clean_detect_roi(existing.get("detectRoi"))
    saved = save_bom_source(config)
    if body.source == "mssql":
        message = "Saved. Inspect queries MSSQL using vision/bom_queries.sql."
    else:
        message = "Saved. Inspect uses vision/sample-bom.json."
    return {"status": "pass", "message": message, **saved}

@app.get("/settings/erp")
def get_erp():
    erp = load_bom_source().get("erp") or _blank_erp()
    return {"name": erp.get("name") or "", "url": erp.get("url") or ""}

@app.get("/settings/erp/frame")
def erp_frame():
    erp = load_bom_source().get("erp") or _blank_erp()
    url = erp.get("url") or ""
    return {
        "name": erp.get("name") or "",
        "url": url,
        "frame": _erp_can_frame(url) if url else False,
    }

@app.post("/settings/erp")
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

@app.get("/settings/scan")
def get_scan():
    seconds = _clean_scan_seconds(load_bom_source().get("scanSeconds"))
    return {"seconds": seconds or DEFAULT_SCAN_SECONDS}

@app.post("/settings/scan")
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

@app.get("/settings/test-mode")
def get_test_mode():
    return {"enabled": load_bom_source().get("testMode") is True}

@app.post("/settings/test-mode")
def update_test_mode(body: TestModeUpdate):
    config = load_bom_source()
    config["testMode"] = body.enabled
    saved = save_bom_source(config)
    if saved["testMode"]:
        message = "Saved. Enter a kit IC to choose an image for detection."
    else:
        message = "Saved. The operator page uses the camera and barcode flow."
    return {"status": "pass", "message": message, "enabled": saved["testMode"]}

@app.get("/settings/detect-roi")
def get_detect_roi():
    return {"roi": _clean_detect_roi(load_bom_source().get("detectRoi"))}

@app.post("/settings/detect-roi")
def update_detect_roi(body: DetectRoiUpdate):
    config = load_bom_source()
    config["detectRoi"] = _clean_detect_roi(None if body.roi is None else body.roi.model_dump())
    saved = save_bom_source(config)
    if saved["detectRoi"]:
        message = "Saved. Detection uses this region."
    else:
        message = "Saved. Detection uses the full image."
    return {"status": "pass", "message": message, "roi": saved["detectRoi"]}

def lookup_bom(barcode):
    source = load_bom_source()
    if source["source"] == "mssql":
        try:
            return _inspect_mssql(barcode, source)
        except Exception as exc:
            return _inspect_fail(barcode, _mssql_error_message(exc, source))

    bom = load_bom()
    job = bom.get(barcode)
    if not job:
        return _inspect_fail(barcode, "Barcode not found in BoM")
    return _inspect_pass(barcode, job)

@app.post("/inspect")
def inspect(req: InspectRequest):
    return lookup_bom(req.barcode)

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8008)
