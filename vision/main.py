import base64
from typing import Optional

import cv2
import numpy as np
from fastapi import Body, FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, field_validator
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

app = FastAPI(title="LS-Vision Sidecar")

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
# config.local.json stores the BoM source and the operator ERP button.
# bom-source.json is read only when config.local.json is absent.
BOM_SOURCE_PATH = Path(__file__).parent / "bom-source.json"
CONFIG_LOCAL_PATH = Path(__file__).parent / "config.local.json"
BOM_QUERIES_PATH = Path(__file__).parent / "bom_queries.sql"
BOM_QUERIES_EXAMPLE_PATH = Path(__file__).parent / "bom_queries.example.sql"
DEFAULT_SQL_DRIVER = "ODBC Driver 18 for SQL Server"
DEFAULT_SCAN_SECONDS = 8
MIN_SCAN_SECONDS = 1
MAX_SCAN_SECONDS = 60
SAMPLE_DIR = Path(__file__).parent / "test-images"
DEFAULT_SAMPLE = "Example3.jpg"
# Areas within this multiple of the group median stay in the same size band.
SIZE_GROUP_RATIO = 2.0
# Min-area-rect aspect is long side / short side.
# Nuts, washers, and isolators are near square. Bolts are long and thin.
ROUND_ASPECT_MAX = 1.6
BOLT_ASPECT_MIN = 2.0
BOLT_ASPECT_MAX = 8.0
# Example3 is about 3 megapixels. This keeps small nuts and washers
# (about 210 px) and still drops the speckle under that.
MIN_AREA_FRACTION = 0.00007
# A filled stack smaller than this is left to the global watershed.
STACK_MIN_AREA = 1500
# Isolator disks sit above this. Stacked bolts are lumpier.
STACK_MAX_CIRCULARITY = 0.62

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
        return _apply_saved_scan(config, saved)
    if BOM_SOURCE_PATH.exists():
        saved = _read_json_dict(BOM_SOURCE_PATH)
        if saved is None:
            return config
        _apply_saved_source(config, saved)
        _apply_saved_erp(config, saved)
        return _apply_saved_scan(config, saved)
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

def _jpeg_data_url(image):
    ok, buffer = cv2.imencode(".jpg", image, [int(cv2.IMWRITE_JPEG_QUALITY), 80])
    if not ok:
        return None
    encoded = base64.b64encode(buffer.tobytes()).decode("ascii")
    return f"data:image/jpeg;base64,{encoded}"

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

def _sample_path(name):
    if not name or Path(name).name != name or name.startswith("."):
        return None
    path = (SAMPLE_DIR / name).resolve()
    try:
        path.relative_to(SAMPLE_DIR.resolve())
    except ValueError:
        return None
    return path

def _size_label(index):
    labels = ["S", "M", "L", "XL", "XXL"]
    if index < len(labels):
        return labels[index]
    return f"S{index + 1}"

def _fill_holes(binary):
    # Flood from a background corner. That fills the bench, not holes inside a part.
    if binary.size == 0 or binary[0, 0] != 0:
        return binary
    flood = binary.copy()
    height, width = binary.shape[:2]
    ff_mask = np.zeros((height + 2, width + 2), np.uint8)
    cv2.floodFill(flood, ff_mask, (0, 0), 255)
    return cv2.bitwise_or(binary, cv2.bitwise_not(flood))

def _split_touching(mask):
    # Open/close cuts thin bridges. Parts that still touch keep separate distance
    # peaks, and watershed cuts between those peaks.
    distance = cv2.distanceTransform(mask, cv2.DIST_L2, 5)
    peak_kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (9, 9))
    dilated = cv2.dilate(distance, peak_kernel)
    peaks = ((distance >= dilated - 1e-3) & (distance >= 4)).astype(np.uint8)
    count, markers = cv2.connectedComponents(peaks)
    if count <= 1:
        return mask
    marks = np.zeros(mask.shape, np.int32)
    marks[peaks > 0] = markers[peaks > 0]
    marks += 1
    marks[(mask > 0) & (peaks == 0)] = 0
    labels = cv2.watershed(cv2.cvtColor(mask, cv2.COLOR_GRAY2BGR), marks)
    split = np.zeros(mask.shape, np.uint8)
    split[labels > 1] = 255
    # A one-pixel watershed line still joins contours on the diagonal.
    cut = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (3, 3))
    return cv2.erode(split, cut)

def _part_shape(contour):
    (_, (rect_w, rect_h), _) = cv2.minAreaRect(contour)
    short_side = min(rect_w, rect_h)
    long_side = max(rect_w, rect_h)
    if short_side < 8:
        return None
    aspect = long_side / float(short_side)
    if aspect <= ROUND_ASPECT_MAX or BOLT_ASPECT_MIN <= aspect <= BOLT_ASPECT_MAX:
        return aspect
    return None

def _circularity(contour):
    area = cv2.contourArea(contour)
    peri = cv2.arcLength(contour, True)
    if peri <= 0 or area <= 0:
        return 0.0
    return float(4.0 * np.pi * area / (peri * peri))

def _bolt_bands(row_strength):
    # Rows where the opened mask is wide are the body of a bolt. The gaps
    # between stacked bolts fall below this level.
    peak = float(row_strength.max()) if row_strength.size else 0.0
    if peak <= 0:
        return []
    level = peak * 0.42
    spans = []
    start = None
    for index, strength in enumerate(row_strength.tolist()):
        if strength >= level and start is None:
            start = index
        elif strength < level and start is not None:
            spans.append([start, index - 1])
            start = None
    if start is not None:
        spans.append([start, row_strength.size - 1])
    merged = []
    for span in spans:
        if merged and span[0] - merged[-1][1] <= 3:
            merged[-1][1] = span[1]
        else:
            merged.append(span)
    return [(band_start, band_end) for band_start, band_end in merged if band_end - band_start + 1 >= 8]

def _split_stacked(opened, mask):
    # Close and hole-fill turn a bolt stack into one solid blob, so the global
    # watershed sees a single peak. The distance transform of the opened mask
    # still has a ridge per bolt. Cutting the valley between ridges separates
    # the stack. A single nick can slice a round part, so the cut is kept only
    # when it yields at least three horizontal bolts.
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    result = mask.copy()
    dilate_kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (5, 5))
    smooth = np.array([1, 2, 3, 2, 1], np.float32)
    smooth /= smooth.sum()
    for contour in contours:
        if cv2.contourArea(contour) < STACK_MIN_AREA or _circularity(contour) >= STACK_MAX_CIRCULARITY:
            continue
        x, y, width, height = [int(value) for value in cv2.boundingRect(contour)]
        if height < 36:
            continue
        component = np.zeros(mask.shape, np.uint8)
        cv2.drawContours(component, [contour], -1, 255, -1)
        local = cv2.bitwise_and(opened, cv2.dilate(component, dilate_kernel))
        crop_open = local[y:y + height, x:x + width]
        crop = component[y:y + height, x:x + width]
        if cv2.countNonZero(crop_open) < 80:
            continue
        distance = cv2.distanceTransform(crop_open, cv2.DIST_L2, 3)
        row_distance = distance.sum(axis=1)
        row_strength = (crop_open > 0).sum(axis=1).astype(np.float32)
        if row_strength.size >= 5:
            row_strength = np.convolve(row_strength, smooth, mode="same")
            row_distance = np.convolve(row_distance, smooth, mode="same")
        bands = _bolt_bands(row_strength)
        if len(bands) < 3:
            continue
        cut = crop.copy()
        for (band_start, band_end), (next_start, _next_end) in zip(bands, bands[1:]):
            if next_start <= band_end:
                continue
            gap = row_distance[band_end:next_start + 1]
            cut_at = int(band_end + np.argmin(gap))
            lo = max(0, cut_at - 1)
            hi = min(cut.shape[0], cut_at + 2)
            cut[lo:hi, :] = 0
        pieces, _ = cv2.findContours(cut, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
        kept = 0
        for piece in pieces:
            piece_area = cv2.contourArea(piece)
            aspect = _part_shape(piece)
            _, _, piece_w, piece_h = cv2.boundingRect(piece)
            if (
                piece_area >= 180
                and aspect is not None
                and aspect >= BOLT_ASPECT_MIN
                and piece_w > piece_h
            ):
                kept += 1
        if kept < 3:
            continue
        region = result[y:y + height, x:x + width]
        region[crop > 0] = 0
        region[cut > 0] = 255
        result[y:y + height, x:x + width] = region
    return result

def _detected_blobs(image):
    gray = cv2.cvtColor(image, cv2.COLOR_BGR2GRAY)
    blur = cv2.GaussianBlur(gray, (7, 7), 0)
    dark = cv2.adaptiveThreshold(
        blur, 255, cv2.ADAPTIVE_THRESH_GAUSSIAN_C, cv2.THRESH_BINARY_INV, 41, 6
    )
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (5, 5))
    # Open drops speckle. Close seals cracks inside a part. Open again so that
    # close cannot glue touching parts back together.
    opened = cv2.morphologyEx(dark, cv2.MORPH_OPEN, kernel)
    mask = cv2.morphologyEx(opened, cv2.MORPH_CLOSE, kernel)
    mask = _fill_holes(mask)
    mask = cv2.morphologyEx(mask, cv2.MORPH_OPEN, kernel)
    mask = _split_touching(mask)
    mask = _split_stacked(opened, mask)
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)

    image_area = gray.shape[0] * gray.shape[1]
    blobs = []
    for contour in contours:
        area = cv2.contourArea(contour)
        if area < image_area * MIN_AREA_FRACTION or area > image_area * 0.02:
            continue
        if _part_shape(contour) is None:
            continue
        x, y, width, height = cv2.boundingRect(contour)
        if width < 1 or height < 1 or area / float(width * height) < 0.2:
            continue
        blobs.append({
            "area": area,
            "x": int(x),
            "y": int(y),
            "w": int(width),
            "h": int(height),
        })
    blobs.sort(key=lambda blob: blob["area"])
    return blobs

def _group_by_size(blobs):
    groups = []
    for blob in blobs:
        area = blob["area"]
        if not groups:
            groups.append([blob])
            continue
        current = groups[-1]
        median = current[len(current) // 2]["area"]
        if area <= median * SIZE_GROUP_RATIO and area <= current[0]["area"] * (SIZE_GROUP_RATIO ** 2):
            current.append(blob)
        else:
            groups.append([blob])
    return groups

def _size_parts(groups):
    descriptions = {
        "S": "Small",
        "M": "Medium",
        "L": "Large",
        "XL": "Extra large",
        "XXL": "Largest",
    }
    parts = []
    for index, group in enumerate(groups):
        label = _size_label(index)
        name = descriptions.get(label, "Size group")
        low = int(round(group[0]["area"]))
        high = int(round(group[-1]["area"]))
        parts.append({
            "ic": label,
            "des": f"{name} blobs, about {low:,}–{high:,} px",
            "qty": len(group),
        })
    return parts

def _annotate_blobs(image, groups):
    annotated = image.copy()
    colors = {
        "S": (0, 170, 0),
        "M": (0, 165, 255),
        "L": (255, 120, 0),
        "XL": (40, 40, 220),
    }
    scale = max(image.shape[0], image.shape[1]) / 1200.0
    thickness = max(2, int(round(scale * 2)))
    font_scale = max(0.45, scale * 0.5)
    for index, group in enumerate(groups):
        label = _size_label(index)
        color = colors.get(label, (255, 255, 255))
        for blob in group:
            x, y, width, height = blob["x"], blob["y"], blob["w"], blob["h"]
            cv2.rectangle(annotated, (x, y), (x + width, y + height), color, thickness)
            (text_width, text_height), baseline = cv2.getTextSize(
                label, cv2.FONT_HERSHEY_SIMPLEX, font_scale, max(1, thickness - 1)
            )
            chip_top = max(0, y - text_height - baseline - 6)
            chip_bottom = chip_top + text_height + baseline + 6
            cv2.rectangle(
                annotated,
                (x, chip_top),
                (x + text_width + 8, chip_bottom),
                color,
                -1,
            )
            cv2.putText(
                annotated,
                label,
                (x + 4, chip_bottom - baseline - 2),
                cv2.FONT_HERSHEY_SIMPLEX,
                font_scale,
                (255, 255, 255),
                max(1, thickness - 1),
                cv2.LINE_AA,
            )
    return _jpeg_data_url(annotated)

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

    blobs = _detected_blobs(image)
    groups = _group_by_size(blobs)
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
        "info": f"Info only: {len(blobs)} blobs counted. Classical contour count, not a BoM check.",
        "image": _annotate_blobs(image, groups),
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
