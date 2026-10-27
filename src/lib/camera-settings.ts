export type CameraScale = {
	mmPerPx: number;
	width: number;
	height: number;
};

const MAX_MM_PER_PX = 1000;

export function readCameraScale(value: unknown): CameraScale | null {
	if (!value || typeof value !== "object" || Array.isArray(value)) return null;
	const data = value as Record<string, unknown>;
	const mmPerPx = Number(data.mmPerPx);
	const width = Number(data.width);
	const height = Number(data.height);
	if (!Number.isFinite(mmPerPx) || mmPerPx <= 0 || mmPerPx > MAX_MM_PER_PX) {
		return null;
	}
	if (!Number.isFinite(width) || width < 1 || width > 20000) return null;
	if (!Number.isFinite(height) || height < 1 || height > 20000) return null;
	return { mmPerPx, width, height };
}

export type CameraSettings = {
	index: number;
	resolution: string;
	displayFps: number;
	focus: number | null;
	brightness: number;
	contrast: number;
	scale: CameraScale | null;
};

export type UvcSettings = {
	autoExposure: boolean;
	exposureAbsolute: number;
	autoFocus: boolean;
	focusAbsolute: number;
	autoWhiteBalance: boolean;
	whiteBalanceKelvin: number;
	gain: number;
};

export function defaultUvcSettings(): UvcSettings {
	return {
		autoExposure: false,
		exposureAbsolute: 156,
		autoFocus: false,
		focusAbsolute: 25,
		autoWhiteBalance: false,
		whiteBalanceKelvin: 4500,
		gain: 0,
	};
}

export function defaultPylonSettings(): {
	exposureTimeUs: number;
	gainDb: number;
	pixelFormat: string;
} {
	return {
		exposureTimeUs: 15000,
		gainDb: 0,
		pixelFormat: "RGB8",
	};
}

const DISPLAY_RATES = [15, 20, 25, 30];

export function readDisplayFps(value: unknown): number {
	const next = Number(value);
	return DISPLAY_RATES.includes(next) ? next : 15;
}

function isModeId(value: string): boolean {
	return /^\d+x\d+@\d+(\.\d{1,3})?$/.test(value);
}

function objectField(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== "object" || Array.isArray(value)) return {};
	return value as Record<string, unknown>;
}

function boolField(value: unknown, fallback: boolean): boolean {
	return typeof value === "boolean" ? value : fallback;
}

function intField(
	value: unknown,
	fallback: number,
	min: number,
	max: number,
): number {
	if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
	const next = Math.round(value);
	if (next < min || next > max) return fallback;
	return next;
}

/** Driver block for the open UVC camera. Missing fields use the bench defaults. */
export function readUvcSettings(camera: unknown): UvcSettings {
	const defaults = defaultUvcSettings();
	const data = objectField(objectField(camera).uvcSettings);
	return {
		autoExposure: boolField(data.autoExposure, defaults.autoExposure),
		exposureAbsolute: intField(
			data.exposureAbsolute,
			defaults.exposureAbsolute,
			-13,
			20000,
		),
		autoFocus: boolField(data.autoFocus, defaults.autoFocus),
		focusAbsolute: intField(data.focusAbsolute, defaults.focusAbsolute, 0, 100),
		autoWhiteBalance: boolField(
			data.autoWhiteBalance,
			defaults.autoWhiteBalance,
		),
		whiteBalanceKelvin: intField(
			data.whiteBalanceKelvin,
			defaults.whiteBalanceKelvin,
			1000,
			15000,
		),
		gain: intField(data.gain, defaults.gain, 0, 255),
	};
}

function levelValue(value: unknown): number | null {
	if (typeof value !== "number" || !Number.isInteger(value)) return null;
	if (value < 0 || value > 100) return null;
	return value;
}

function levels(settings: CameraSettings): Record<string, unknown> {
	const record: Record<string, unknown> = {
		resolution: settings.resolution,
		displayFps: settings.displayFps,
		focus: settings.focus,
		brightness: settings.brightness,
		contrast: settings.contrast,
	};
	// Leave scale out when it is unset so the stored record can keep its own.
	if (settings.scale) record.scale = settings.scale;
	return record;
}

function storedRecord(
	byIndex: Record<string, unknown>,
	key: string,
	settings: CameraSettings,
): Record<string, unknown> {
	const previous = objectField(byIndex[key]);
	const next = { ...previous, ...levels(settings) };
	if (!settings.scale && Object.hasOwn(previous, "scale")) {
		next.scale = previous.scale;
	}
	return next;
}

export function defaultCameraSettings(index: number): CameraSettings {
	return {
		index,
		resolution: "",
		displayFps: 15,
		focus: null,
		brightness: 50,
		contrast: 50,
		scale: null,
	};
}

function settingsFrom(value: unknown, index: number): CameraSettings {
	const data = objectField(value);
	const resolution = typeof data.resolution === "string" ? data.resolution : "";
	return {
		index,
		resolution: isModeId(resolution) ? resolution : "",
		displayFps: readDisplayFps(data.displayFps),
		focus: Object.hasOwn(data, "focus") ? levelValue(data.focus) : null,
		brightness: levelValue(data.brightness) ?? 50,
		contrast: levelValue(data.contrast) ?? 50,
		scale: readCameraScale(data.scale),
	};
}

/** Saved values for one camera. A camera with no record uses the defaults.
 * The old single `camera` object belongs only to its `index`. */
export function settingsForCamera(
	camera: unknown,
	index: number,
): CameraSettings {
	const data = objectField(camera);
	const byIndex = objectField(data.byIndex);
	const key = String(index);
	if (Object.hasOwn(byIndex, key)) return settingsFrom(byIndex[key], index);
	const active = Number(data.index);
	if (Number.isInteger(active) && active === index)
		return settingsFrom(data, index);
	return defaultCameraSettings(index);
}

/** Keep the current top-level fields under that camera before they are replaced. */
export function rememberActiveCamera(document: Record<string, unknown>): void {
	const camera = objectField(document.camera);
	const index = Number(camera.index);
	if (!Number.isInteger(index) || index < 0 || index > 32) return;
	const byIndex = objectField(camera.byIndex);
	const key = String(index);
	if (Object.hasOwn(byIndex, key)) return;
	byIndex[key] = levels(settingsFrom(camera, index));
	camera.byIndex = byIndex;
	document.camera = camera;
}

function writeCamera(
	document: Record<string, unknown>,
	settings: CameraSettings,
	active: boolean,
): void {
	const camera = objectField(document.camera);
	const byIndex = objectField(camera.byIndex);
	const key = String(settings.index);
	byIndex[key] = storedRecord(byIndex, key, settings);
	if (!active) {
		camera.byIndex = byIndex;
		document.camera = camera;
		return;
	}
	const next: Record<string, unknown> = {
		...camera,
		...levels(settings),
		index: settings.index,
		deviceIndex: settings.index,
		byIndex,
	};
	if (typeof next.type !== "string" || next.type.trim() === "")
		next.type = "uvc";
	if (
		!next.uvcSettings ||
		typeof next.uvcSettings !== "object" ||
		Array.isArray(next.uvcSettings)
	) {
		next.uvcSettings = defaultUvcSettings();
	}
	if (
		!next.pylonSettings ||
		typeof next.pylonSettings !== "object" ||
		Array.isArray(next.pylonSettings)
	) {
		next.pylonSettings = defaultPylonSettings();
	}
	document.camera = next;
}

/** Store `settings` without changing which camera is selected. */
export function retainCameraSettings(
	document: Record<string, unknown>,
	settings: CameraSettings,
): void {
	writeCamera(document, settings, false);
}

/** Make `settings` the active camera and store them on that camera's record. */
export function assignActiveCamera(
	document: Record<string, unknown>,
	settings: CameraSettings,
): void {
	writeCamera(document, settings, true);
}

/** Select `index` and keep the camera that was active. */
export function activateCamera(
	document: Record<string, unknown>,
	index: number,
): CameraSettings {
	rememberActiveCamera(document);
	const settings = settingsForCamera(objectField(document.camera), index);
	assignActiveCamera(document, settings);
	return settings;
}
