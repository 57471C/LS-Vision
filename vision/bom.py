import json
import re
import urllib.error
import urllib.request
from decimal import Decimal
from pathlib import Path
from urllib.parse import urlparse

from fastapi import APIRouter
from fastapi.responses import JSONResponse
from pydantic import BaseModel, field_validator

from barcode import log
from config import DEFAULT_SQL_DRIVER, load_bom_source

BOM_PATH = Path(__file__).parent / "sample-bom.json"
BOM_QUERIES_PATH = Path(__file__).parent / "bom_queries.sql"
BOM_QUERIES_EXAMPLE_PATH = Path(__file__).parent / "bom_queries.example.sql"

class InspectRequest(BaseModel):
    barcode: str

class BomApiRequest(BaseModel):
    jobCardNumber: str | int

    @field_validator("jobCardNumber", mode="before")
    @classmethod
    def job_card_value(cls, value):
        # JSON true must not become a job card number.
        if isinstance(value, bool) or value is None:
            raise ValueError("Enter a job card number.")
        return value

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

API_TIMEOUT_SECONDS = 15

def _join_api_url(base_url, path):
    base = (base_url or "").strip().rstrip("/")
    suffix = (path or "").strip()
    if suffix and not suffix.startswith("/"):
        suffix = "/" + suffix
    return base + suffix

def _job_card_number(value):
    text = _as_text(value)
    if not text.isdigit() or len(text) > 18:
        return None, "Enter a job card number."
    return int(text), None

def _api_field(row, *names):
    if not isinstance(row, dict):
        return None
    lookup = {str(key).lower(): value for key, value in row.items()}
    for name in names:
        if name.lower() in lookup:
            return lookup[name.lower()]
    return None

def _job_record_from_api(row):
    raw_parts = _api_field(row, "parts")
    if not isinstance(raw_parts, list):
        return None, "BoM API response has no parts."
    parts = []
    for part in raw_parts:
        ic = _as_text(_api_field(part, "ic", "cc"))
        if not ic:
            continue
        qty = _as_number(_api_field(part, "qty", "quantity"))
        parts.append(
            {
                "ic": ic,
                "des": _as_text(_api_field(part, "des", "description")),
                "qty": 0 if qty is None else qty,
            }
        )
    expected = _api_field(row, "expectedKitCount", "expectedkitcount", "labelCount")
    return {
        "des": _as_text(_api_field(row, "des", "description", "kitName")),
        "expectedKitCount": None if expected is None else _as_number(expected),
        "parts": parts,
    }, None

def _kits_keyed_by_ic(data):
    # The BoM API uses the same shape as sample-bom.json: { kitIC: { des, parts } }.
    kits = []
    for key, value in data.items():
        if not isinstance(value, dict):
            continue
        if not isinstance(_api_field(value, "parts"), list):
            continue
        kit_ic = _as_text(key)
        if kit_ic:
            kits.append((kit_ic, value))
    return kits

def _job_from_api_data(data):
    # The API data object becomes the same kit shape as the JSON and SQL BoMs.
    if not isinstance(data, dict):
        return None, None, "BoM API response has no data object."
    kit_ic = _as_text(_api_field(data, "kitIC", "kitIc", "kitCode", "kit", "ic"))
    if kit_ic and isinstance(_api_field(data, "parts"), list):
        job, error = _job_record_from_api(data)
        if error:
            return None, None, error
        return job, kit_ic, None
    kits = _kits_keyed_by_ic(data)
    if len(kits) > 1:
        return None, None, "BoM API returned more than one kit."
    if len(kits) == 1:
        job, error = _job_record_from_api(kits[0][1])
        if error:
            return None, None, error
        return job, kits[0][0], None
    if kit_ic:
        return None, None, "BoM API response has no parts."
    return None, None, "BoM API response has no kit IC."

def _payload_message(payload):
    if not isinstance(payload, dict):
        return ""
    for name in ("message", "error", "title", "detail", "status"):
        text = _as_text(_api_field(payload, name))
        if not text:
            continue
        # A success flag is not the error text. A sentence in status is.
        if name == "status" and text.lower() in {"ok", "success", "pass", "true", "200"}:
            continue
        return text
    return ""

def _api_error_detail(exc, key):
    try:
        raw = exc.read()
    except Exception:
        return ""
    try:
        payload = json.loads(raw.decode("utf-8"))
    except Exception:
        text = raw.decode("utf-8", errors="replace").strip()
        return _scrub_secret(text, key)[:200]
    detail = _payload_message(payload)
    if not detail:
        return ""
    return _scrub_secret(detail, key)[:200]

class BomApiError(Exception):
    def __init__(self, status, message):
        super().__init__(message)
        self.status = status

def _failure_status(code):
    if isinstance(code, int) and 400 <= code <= 599:
        return code
    return 502

def _post_bom_api(url, key, number):
    body = json.dumps({"jobCardNumber": number}).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
        method="POST",
    )
    # Request.add_header capitalizes names, which turns X-Api-Key into X-api-key.
    request.headers["X-Api-Key"] = key
    try:
        with urllib.request.urlopen(request, timeout=API_TIMEOUT_SECONDS) as response:
            status = getattr(response, "status", 200)
            raw = response.read()
    except urllib.error.HTTPError as exc:
        detail = _api_error_detail(exc, key)
        message = f"BoM API returned HTTP {exc.code}."
        if detail:
            message = f"{message} {detail}"
        raise BomApiError(_failure_status(exc.code), message) from exc
    except urllib.error.URLError as exc:
        reason = getattr(exc, "reason", exc)
        if isinstance(reason, TimeoutError):
            raise BomApiError(504, "The BoM API did not respond in time.") from exc
        text = _scrub_secret(str(reason).strip() or "connection failed", key)
        raise BomApiError(502, f"Could not reach the BoM API. {text}") from exc
    except TimeoutError as exc:
        raise BomApiError(504, "The BoM API did not respond in time.") from exc
    if status != 200:
        raise BomApiError(_failure_status(status), f"BoM API returned HTTP {status}.")
    try:
        payload = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise BomApiError(502, "BoM API returned invalid JSON.") from exc
    if not isinstance(payload, dict) or not isinstance(payload.get("data"), dict):
        detail = _scrub_secret(_payload_message(payload), key)[:200]
        if detail:
            raise BomApiError(422, detail)
        raise BomApiError(502, "BoM API response has no data object.")
    return payload["data"]

def _api_bom_data(job_card, source):
    number, error = _job_card_number(job_card)
    if error:
        raise BomApiError(400, error)
    api = source.get("api") if isinstance(source.get("api"), dict) else {}
    key = _as_text(api.get("key"))
    base_url = _as_text(api.get("baseUrl"))
    path = _as_text(api.get("path"))
    if not key:
        raise BomApiError(400, "API key is not set. Add it in Settings.")
    if not base_url:
        raise BomApiError(400, "API base URL is not set. Add it in Settings.")
    parsed = urlparse(base_url)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise BomApiError(400, "API base URL must start with http:// or https://.")
    try:
        return _post_bom_api(_join_api_url(base_url, path), key, number)
    except BomApiError:
        raise
    except Exception as exc:
        text = _scrub_secret(str(exc).strip() or exc.__class__.__name__, key)
        raise BomApiError(502, f"BoM API lookup failed. {text}") from exc

def _inspect_api(job_card, source):
    try:
        data = _api_bom_data(job_card, source)
    except BomApiError as exc:
        return _inspect_fail(job_card, str(exc))
    job, kit_ic, error = _job_from_api_data(data)
    if error:
        return _inspect_fail(job_card, error)
    result = _inspect_pass(kit_ic, job)
    result["data"] = data
    # The pass payload's barcode is the kit IC. Keep the number that was looked up.
    result["jobCardNumber"] = _as_text(job_card)
    return result

def load_bom():
    if not BOM_PATH.exists():
        return {}
    with open(BOM_PATH, encoding="utf-8") as f:
        return json.load(f)

router = APIRouter()

def lookup_bom(barcode):
    source = load_bom_source()
    if source["source"] == "mssql":
        try:
            return _inspect_mssql(barcode, source)
        except Exception as exc:
            return _inspect_fail(barcode, _mssql_error_message(exc, source))
    if source["source"] == "api":
        return _inspect_api(barcode, source)

    bom = load_bom()
    job = bom.get(barcode)
    if not job:
        return _inspect_fail(barcode, "Barcode not found in BoM")
    return _inspect_pass(barcode, job)

@router.post("/inspect")
def inspect(req: InspectRequest):
    kind = load_bom_source().get("source") or "json"
    log.info("path manual: typed code %s source=%s", req.barcode, kind)
    return lookup_bom(req.barcode)

@router.post("/bom/api")
def bom_api(body: BomApiRequest):
    # baseUrl, path, and key come from config.local.json, not the request.
    log.info("path manual: API job card %s", body.jobCardNumber)
    try:
        return _api_bom_data(body.jobCardNumber, load_bom_source())
    except BomApiError as exc:
        return JSONResponse(
            status_code=exc.status,
            content={"message": str(exc)},
        )
