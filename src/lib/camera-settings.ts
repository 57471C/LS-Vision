export type CameraSettings = {
	index: number;
	resolution: string;
	displayFps: number;
	focus: number;
	brightness: number;
	contrast: number;
};

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

function levelValue(value: unknown): number | null {
	if (typeof value !== "number" || !Number.isInteger(value)) return null;
	if (value < 0 || value > 100) return null;
	return value;
}

function levels(settings: CameraSettings): Record<string, unknown> {
	return {
		resolution: settings.resolution,
		displayFps: settings.displayFps,
		focus: settings.focus,
		brightness: settings.brightness,
		contrast: settings.contrast,
	};
}

export function defaultCameraSettings(index: number): CameraSettings {
	return {
		index,
		resolution: "",
		displayFps: 15,
		focus: 50,
		brightness: 50,
		contrast: 50,
	};
}

function settingsFrom(value: unknown, index: number): CameraSettings {
	const data = objectField(value);
	const resolution = typeof data.resolution === "string" ? data.resolution : "";
	return {
		index,
		resolution: isModeId(resolution) ? resolution : "",
		displayFps: readDisplayFps(data.displayFps),
		focus: levelValue(data.focus) ?? 50,
		brightness: levelValue(data.brightness) ?? 50,
		contrast: levelValue(data.contrast) ?? 50,
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
	byIndex[String(settings.index)] = levels(settings);
	if (!active) {
		camera.byIndex = byIndex;
		document.camera = camera;
		return;
	}
	document.camera = {
		...camera,
		...levels(settings),
		index: settings.index,
		byIndex,
	};
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
