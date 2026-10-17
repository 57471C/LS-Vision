import base64
import ctypes
import os
import sys
import threading
import time
from typing import Optional

import barcode
from barcode import _codes_from_frame, _pyzbar_for_decode, _pyzbar_load_error, log
import cv2

from fastapi import APIRouter
from fastapi.responses import StreamingResponse

from bom import lookup_bom
from config import load_bom_source

def _camera_thread_excepthook(args):
    name = args.thread.name if args.thread is not None else ""
    if name.startswith("ls-vision-camera"):
        log.error(
            "camera thread crashed",
            exc_info=(args.exc_type, args.exc_value, args.exc_traceback),
        )
        return
    sys.__excepthook__(args.exc_type, args.exc_value, args.exc_traceback)

threading.excepthook = _camera_thread_excepthook

# The reader thread is the only owner of the device. The MJPEG stream and the
# barcode routes copy the latest frame, so a decode cannot stall or close the stream.
camera = None
_camera_index = None
_frame_lock = threading.Lock()
_capture_lock = threading.Lock()
_capture_thread = None
_capture_stop = None
_capture_generation = 0
_capture_target = None
_capture_error = None
_latest_frame = None
_latest_frame_id = 0
_frames_published = 0
_OPEN_RETRY_SECONDS = 0.3
_SWITCH_SETTLE_SECONDS = 0.2
_FRAME_WAIT_SECONDS = 5.0
# Bench cameras use DirectShow indexes 0 through 5. Three misses in a row
# means the higher indexes are empty too, so the scan stops there.
_CAMERA_PROBE_LAST = 5
_CAMERA_PROBE_MISS_LIMIT = 3
# Last raw frame a snapshot held. POST /detect uses it when
# the request does not include an image.
_latest_snapshot = None
_controls_lock = threading.Lock()
_pending_controls = None

def _release_cap(cap):
    if cap is None:
        return
    try:
        cap.release()
    except Exception:
        log.exception("camera release failed")

def _init_capture_com():
    if os.name != "nt":
        return
    # DirectShow capture from a packaged GUI exe stays black or stalls unless
    # the thread that opens the device is an STA. Dev still uses CAP_DSHOW.
    try:
        hr = ctypes.windll.ole32.CoInitializeEx(None, 0x2)
    except Exception:
        log.exception("CoInitializeEx failed")
        return
    log.info("CoInitializeEx STA hr=0x%08x", hr & 0xFFFFFFFF)

def _open_capture(index):
    # Same open as dev: CAP_DSHOW. A second attempt covers a device that is
    # still releasing. There is no MSMF fallback; that backend is black for one
    # of the bench cameras in the packaged exe.
    last_error = f"Camera {index} did not open (CAP_DSHOW)"
    for attempt in (1, 2):
        cap = None
        try:
            cap = cv2.VideoCapture(int(index), cv2.CAP_DSHOW)
            if cap is not None and cap.isOpened():
                width = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH) or 0)
                height = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT) or 0)
                log.info(
                    "camera %s opened via CAP_DSHOW on attempt %s (%sx%s)",
                    index,
                    attempt,
                    width,
                    height,
                )
                return cap, None
            last_error = f"Camera {index} did not open (CAP_DSHOW)"
            log.warning("%s on attempt %s", last_error, attempt)
        except Exception as exc:
            last_error = f"Camera {index} failed to open: {exc}"
            log.exception("camera %s open failed on attempt %s", index, attempt)
        _release_cap(cap)
        cap = None
        if attempt == 1:
            time.sleep(_OPEN_RETRY_SECONDS)
    return None, last_error

_SPAN_GUESSES = (255, 1023, 2047, 4095, 8191)
_control_spans = {}
_unsupported_logged = set()

def _discover_span(cap, prop):
    # The UI is 0 to 100. DirectShow cameras use their own maximum, so find
    # the highest value this device keeps, then put the lens back.
    current = cap.get(prop)
    found = None
    try:
        for guess in _SPAN_GUESSES:
            if not cap.set(prop, float(guess)):
                if found is None:
                    continue
                break
            read = float(cap.get(prop))
            if abs(read - guess) <= 1.5:
                found = guess
                continue
            if abs(read - float(current)) > 1.5:
                found = max(int(round(read)), 1)
            break
    finally:
        try:
            cap.set(prop, current)
        except Exception:
            log.exception("camera control restore failed")
    return found

def _span_for(cap, index, prop):
    key = (int(index), int(prop))
    if key in _control_spans:
        return _control_spans[key]
    span = _discover_span(cap, prop)
    _control_spans[key] = span
    return span

def _set_level(cap, prop, level, label, index):
    if isinstance(level, bool) or not isinstance(level, int):
        return
    span = _span_for(cap, index, prop)
    if not span:
        key = (int(index), int(prop))
        if key not in _unsupported_logged:
            _unsupported_logged.add(key)
            log.info("camera %s is not supported", label)
        return
    applied = int(level * span / 100 + 0.5)
    if not cap.set(prop, float(applied)):
        log.info("camera %s %s was not accepted", label, level)
        return
    log.info(
        "camera %s ui=%s applied=%s of %s read=%s",
        label,
        level,
        applied,
        span,
        cap.get(prop),
    )

def _apply_controls(cap, controls, index):
    if cap is None or not isinstance(controls, dict):
        return
    try:
        resolution = controls.get("resolution") or ""
        if isinstance(resolution, str) and "x" in resolution:
            width_text, height_text = resolution.lower().split("x", 1)
            cap.set(cv2.CAP_PROP_FRAME_WIDTH, float(width_text))
            cap.set(cv2.CAP_PROP_FRAME_HEIGHT, float(height_text))
            log.info(
                "camera size set %s read %sx%s",
                resolution,
                int(cap.get(cv2.CAP_PROP_FRAME_WIDTH) or 0),
                int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT) or 0),
            )
        _set_level(cap, cv2.CAP_PROP_BRIGHTNESS, controls.get("brightness"), "brightness", index)
        _set_level(cap, cv2.CAP_PROP_CONTRAST, controls.get("contrast"), "contrast", index)
        focus = controls.get("focus")
        if isinstance(focus, int) and not isinstance(focus, bool):
            if hasattr(cv2, "CAP_PROP_AUTOFOCUS"):
                cap.set(cv2.CAP_PROP_AUTOFOCUS, 0)
            _set_level(cap, cv2.CAP_PROP_FOCUS, focus, "focus", index)
    except Exception:
        log.exception("camera controls were not applied")

def _saved_controls():
    camera_settings = load_bom_source().get("camera") or {}
    if not isinstance(camera_settings, dict):
        return {}
    return {
        "resolution": camera_settings.get("resolution") or "",
        "focus": camera_settings.get("focus"),
        "brightness": camera_settings.get("brightness"),
        "contrast": camera_settings.get("contrast"),
    }

def publish_camera_controls(camera_settings):
    # The reader thread applies these on its next frame. This does not open
    # a second capture.
    if not isinstance(camera_settings, dict):
        return
    try:
        index = int(camera_settings.get("index"))
    except (TypeError, ValueError):
        return
    if _active_reader_index() != index:
        return
    controls = {
        "resolution": camera_settings.get("resolution") or "",
        "focus": camera_settings.get("focus"),
        "brightness": camera_settings.get("brightness"),
        "contrast": camera_settings.get("contrast"),
    }
    global _pending_controls
    with _controls_lock:
        _pending_controls = (index, controls)

def _take_pending(index):
    global _pending_controls
    with _controls_lock:
        pending = _pending_controls
        if pending is None or pending[0] != int(index):
            return None
        _pending_controls = None
        return pending[1]

def _active_reader_index():
    # The reader thread owns this index, including while the first open is
    # still in progress. A second VideoCapture on it fails and drops the feed.
    with _capture_lock:
        thread = _capture_thread
        target = _capture_target
        alive = thread is not None and thread.is_alive()
    if not alive or target is None:
        return None
    try:
        return int(target)
    except (TypeError, ValueError):
        return None

def _camera_opens(index):
    # One DirectShow open, the same backend the reader uses. There is no MSMF
    # fallback; that backend stays black for one of the bench cameras.
    cap = cv2.VideoCapture()
    opened = False
    try:
        if hasattr(cv2, "CAP_PROP_OPEN_TIMEOUT_MSEC"):
            cap.set(cv2.CAP_PROP_OPEN_TIMEOUT_MSEC, 2000)
        cap.open(int(index), cv2.CAP_DSHOW)
        opened = bool(cap.isOpened())
    except Exception:
        log.exception("camera %s probe failed", index)
        return False
    finally:
        _release_cap(cap)
    if opened:
        log.info("camera %s probe opened", index)
    else:
        log.info("camera %s probe did not open", index)
    return opened

def _discover_camera_indices(opens, held=None, selected=None):
    # `opens(index)` probes one index. `held` is not probed. `selected` is the
    # operator's current index; if the miss streak stops the scan before it,
    # that index is still probed so a working choice stays in the list.
    available = []
    misses = 0
    visited = set()
    for index in range(_CAMERA_PROBE_LAST + 1):
        visited.add(index)
        if held is not None and index == held:
            available.append(index)
            misses = 0
            continue
        if opens(index):
            available.append(index)
            misses = 0
        else:
            misses += 1
            if misses >= _CAMERA_PROBE_MISS_LIMIT:
                break
    if held is not None and held not in available:
        available.append(held)
    if (
        selected is not None
        and selected not in available
        and selected not in visited
        and opens(selected)
    ):
        available.append(selected)
    return sorted(set(available))

def _stop_reader_locked():
    global _capture_thread, _capture_target, camera, _camera_index, _latest_frame, _frames_published
    stop = _capture_stop
    thread = _capture_thread
    was_running = thread is not None and thread.is_alive()
    if stop is not None:
        stop.set()
    if was_running and thread is not threading.current_thread():
        thread.join(timeout=2.5)
        if thread.is_alive():
            log.warning("camera reader is still stopping; releasing the open device")
            with _frame_lock:
                stuck = camera
                camera = None
                _camera_index = None
                _latest_frame = None
                _frames_published = 0
            _release_cap(stuck)
            thread.join(timeout=1.0)
            if thread.is_alive():
                log.error("camera reader did not stop")
    _capture_thread = None
    _capture_target = None
    return was_running

def _read_capture(cap, index, stop, generation):
    global camera, _camera_index, _latest_frame, _latest_frame_id, _frames_published, _capture_error
    with _frame_lock:
        if generation != _capture_generation or stop.is_set():
            return
        camera = cap
        _camera_index = index
        _capture_error = None
    empty_logged = False
    read_error_at = 0.0
    while not stop.is_set() and generation == _capture_generation:
        pending = _take_pending(index)
        if pending is not None:
            _apply_controls(cap, pending, index)
        try:
            ok, frame = cap.read()
        except Exception:
            now = time.monotonic()
            if now - read_error_at > 2:
                log.exception("camera %s read failed", index)
                read_error_at = now
            time.sleep(0.05)
            continue
        if not ok or frame is None:
            if not empty_logged:
                log.warning("camera %s returned an empty frame", index)
                empty_logged = True
            time.sleep(0.02)
            continue
        empty_logged = False
        published = frame.copy()
        with _frame_lock:
            if generation != _capture_generation:
                return
            first = _frames_published == 0
            _latest_frame = published
            _latest_frame_id += 1
            _frames_published += 1
        if first and not published.any():
            log.warning("camera %s first frame is black", index)

def _capture_loop(index, stop, generation):
    global camera, _camera_index, _latest_frame, _frames_published, _capture_error
    _init_capture_com()
    logged_failure = False
    try:
        while not stop.is_set() and generation == _capture_generation:
            cap, error = _open_capture(index)
            if stop.is_set() or generation != _capture_generation:
                _release_cap(cap)
                log.info("camera %s open cancelled", index)
                return
            if cap is None:
                with _frame_lock:
                    if generation == _capture_generation:
                        _capture_error = error
                if not logged_failure:
                    log.error("camera %s unavailable: %s", index, error)
                    logged_failure = True
                else:
                    log.info("camera %s still unavailable: %s", index, error)
                if stop.wait(1.0):
                    return
                continue
            logged_failure = False
            with _frame_lock:
                if generation == _capture_generation:
                    _capture_error = None
            _apply_controls(cap, _saved_controls(), index)
            try:
                _read_capture(cap, index, stop, generation)
            except Exception:
                log.exception("camera %s read loop failed", index)
                with _frame_lock:
                    if generation == _capture_generation:
                        _capture_error = f"Camera {index} read failed"
            finally:
                _release_cap(cap)
                with _frame_lock:
                    if _capture_generation == generation and camera is cap:
                        camera = None
                        _camera_index = None
                        _latest_frame = None
                        _frames_published = 0
                log.info("camera %s released", index)
            if stop.is_set() or generation != _capture_generation:
                return
            if stop.wait(0.5):
                return
    except Exception:
        log.exception("camera %s reader crashed", index)
        with _frame_lock:
            if generation == _capture_generation:
                _capture_error = f"Camera {index} failed"

def ensure_capture(index):
    global _capture_generation, _capture_stop, _capture_target, _capture_error, _capture_thread
    global _latest_frame, _frames_published
    index = int(index)
    with _capture_lock:
        if (
            _capture_thread is not None
            and _capture_thread.is_alive()
            and _capture_target == index
        ):
            return
        released = _stop_reader_locked()
        if released:
            # DirectShow keeps the previous device black if the next open overlaps it.
            time.sleep(_SWITCH_SETTLE_SECONDS)
        _capture_generation += 1
        generation = _capture_generation
        stop = threading.Event()
        _capture_stop = stop
        _capture_target = index
        _capture_error = None
        with _frame_lock:
            _latest_frame = None
            _frames_published = 0
        thread = threading.Thread(
            target=_capture_loop,
            args=(index, stop, generation),
            name=f"ls-vision-camera-{index}",
            daemon=True,
        )
        _capture_thread = thread
        thread.start()
        log.info("camera %s reader started", index)

def _shutdown_capture():
    with _capture_lock:
        _stop_reader_locked()

def _frame_or_error(index, min_frames=1):
    index = int(index)
    try:
        ensure_capture(index)
    except Exception:
        log.exception("camera %s setup failed", index)
        return None, f"Camera {index} failed to open"
    deadline = time.monotonic() + _FRAME_WAIT_SECONDS
    while time.monotonic() < deadline:
        with _frame_lock:
            if (
                _capture_target == index
                and _latest_frame is not None
                and _frames_published >= min_frames
            ):
                return _latest_frame.copy(), None
            error = _capture_error
            published = _frames_published
        # The reader keeps retrying, so a failed open must not sit here for the
        # whole wait. The live stream stays connected and reads the next frame.
        if error and published == 0:
            return None, error
        time.sleep(0.02)
    with _frame_lock:
        error = _capture_error
    message = error or "Failed to capture a frame"
    log.warning("camera %s had no frame: %s", index, message)
    return None, message

def _capture_frame(index):
    # First frames from many webcams are blank. Wait until the reader has a few.
    return _frame_or_error(index, min_frames=5)

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

def _take_stream_frame(index, last_id):
    with _frame_lock:
        if _capture_target != int(index) or _latest_frame is None or _latest_frame_id == last_id:
            return None
        frame = _latest_frame
        frame_id = _latest_frame_id
    try:
        ok, jpeg = cv2.imencode(".jpg", frame)
    except Exception:
        log.exception("camera %s stream encode failed", index)
        time.sleep(0.2)
        return None
    if not ok:
        log.warning("camera %s stream encode returned no jpeg", index)
        time.sleep(0.2)
        return None
    payload = (
        b"--frame\r\n"
        b"Content-Type: image/jpeg\r\n\r\n" + jpeg.tobytes() + b"\r\n"
    )
    return payload, frame_id

def _reopen_if_mine(index):
    # A stream that the operator has already left must not grab the device back.
    index = int(index)
    with _capture_lock:
        mine = _capture_target == index
        alive = _capture_thread is not None and _capture_thread.is_alive()
    if mine and not alive:
        ensure_capture(index)

def iter_camera_stream(index):
    # Decode never touches this generator. A miss, a bad frame, or a pyzbar
    # error must not end the multipart response. The device is claimed once
    # per connection so a stale response cannot reopen the previous camera.
    log.info("camera %s stream connected", index)
    last_id = -1
    try:
        try:
            ensure_capture(index)
        except Exception:
            log.exception("camera %s stream could not open", index)
        while True:
            try:
                _reopen_if_mine(index)
                item = _take_stream_frame(index, last_id)
            except Exception:
                log.exception("camera %s stream failed", index)
                time.sleep(0.2)
                continue
            if item is None:
                time.sleep(0.02)
                continue
            payload, frame_id = item
            last_id = frame_id
            yield payload
    finally:
        log.info("camera %s stream closed", index)

router = APIRouter()

@router.get("/camera/stream")
def camera_stream(index: int = 0):
    return StreamingResponse(
        iter_camera_stream(index),
        media_type="multipart/x-mixed-replace; boundary=frame",
    )

@router.get("/camera/decode")
def camera_decode(index: int = 0):
    try:
        return _camera_decode(index)
    except Exception:
        log.exception("camera decode failed")
        return {"status": "fail", "message": "Barcode decode failed", "codes": []}

def _camera_decode(index):
    frame, error = _frame_or_error(index)
    if frame is None:
        return {"status": "fail", "message": error or "No frame"}

    if _pyzbar_for_decode() is None:
        log.warning("barcode decode failed: %s", _pyzbar_load_error())
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

@router.get("/camera/snapshot")
def camera_snapshot(index: int = 0, frame: bool = False):
    try:
        return _camera_snapshot(index, frame_only=frame)
    except Exception:
        log.exception("camera snapshot failed")
        return {"status": "fail", "snapshot": False, "message": "Snapshot failed"}

def _camera_snapshot(index, frame_only=False):
    # Test mode is the manual path: typed code and an image file, no camera decode.
    # Otherwise every source decodes. API uses that code as the job card number.
    # JSON and MSSQL look the same code up as a kit IC.
    # frame=1 returns that same reader frame as a JPEG and skips pyzbar and the
    # BoM lookup. The plain route still decodes. Neither path calls cap.read().
    config = load_bom_source()
    if config.get("testMode") is True:
        log.info("path manual: test mode, skip barcode decode")
        return {
            "status": "idle",
            "snapshot": False,
            "message": "Test mode uses the typed code.",
        }

    if frame_only:
        log.info("path barcode: frame only")
        frame, error = _frame_or_error(index)
        if frame is None:
            return {"status": "fail", "snapshot": False, "message": error or "No frame"}
        image = _jpeg_data_url(frame)
        if not image:
            return {
                "status": "fail",
                "snapshot": False,
                "message": "Could not encode the frame",
            }
        return {"status": "pass", "snapshot": False, "image": image}

    kind = config.get("source") or "json"
    log.info("path barcode: source=%s", kind)

    # Freeze this frame only when its lookup passes.
    # The copy comes from the reader thread. This route does not call cap.read().
    frame, error = _frame_or_error(index)
    if frame is None:
        return {"status": "fail", "snapshot": False, "message": error or "No frame"}

    if _pyzbar_for_decode() is None:
        log.warning("barcode decode failed: %s", _pyzbar_load_error())
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
    if kind == "api":
        log.info("path barcode: API job card %s", barcode)
    else:
        log.info("path barcode: BoM lookup %s", barcode)
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

@router.get("/cameras")
def cameras(selected: Optional[int] = None):
    # The probe opens DirectShow devices. Match the reader and use an STA.
    _init_capture_com()
    if selected is not None and selected < 0:
        selected = None
    held = _active_reader_index()
    found = _discover_camera_indices(_camera_opens, held=held, selected=selected)
    log.info("cameras found %s (held=%s selected=%s)", found, held, selected)
    return {
        "cameras": found,
        "active": held if held is not None else barcode.CAMERA_INDEX,
    }
