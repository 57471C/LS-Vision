// Straight-line measure and per-camera scale.
// The stored factor is millimetres per pixel of the image that was calibrated.
// A later image of the same view is scaled by its width, so a full-resolution
// snapshot and the 1280×720 preview agree.

import type { CameraScale } from "./camera-settings";

export type { CameraScale };

export type ImagePoint = { x: number; y: number };

export type ImageLine = {
	x1: number;
	y1: number;
	x2: number;
	y2: number;
};

export type FrameBox = {
	left: number;
	top: number;
	width: number;
	height: number;
};

export type MeasureDraw = {
	line: ImageLine;
	width: number;
	height: number;
};

export type MeasureReadout = {
	pixels: string;
	length: string | null;
	message: string | null;
};

export const MIN_LINE_PX = 8;
const MAX_LENGTH_MM = 10000;
const MAX_MM_PER_PX = 1000;

export function lineLength(line: ImageLine): number {
	return Math.hypot(line.x2 - line.x1, line.y2 - line.y1);
}

export function imagePoint(
	clientX: number,
	clientY: number,
	rect: { left: number; top: number; width: number; height: number },
	imageWidth: number,
	imageHeight: number,
): ImagePoint | null {
	if (rect.width < 1 || rect.height < 1) return null;
	if (imageWidth < 1 || imageHeight < 1) return null;
	const x = ((clientX - rect.left) / rect.width) * imageWidth;
	const y = ((clientY - rect.top) / rect.height) * imageHeight;
	if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
	return {
		x: Math.min(imageWidth, Math.max(0, x)),
		y: Math.min(imageHeight, Math.max(0, y)),
	};
}

/** A finished stroke. A line shorter than {@link MIN_LINE_PX} is discarded. */
export function finishLine(
	origin: ImagePoint,
	point: ImagePoint,
): ImageLine | null {
	const line = { x1: origin.x, y1: origin.y, x2: point.x, y2: point.y };
	if (lineLength(line) < MIN_LINE_PX) return null;
	return line;
}

export function parseMillimetres(text: string): number | null {
	const cleaned = text.trim().replace(",", ".");
	if (!/^\d+(\.\d+)?$/.test(cleaned)) return null;
	const value = Number(cleaned);
	if (!Number.isFinite(value) || value <= 0 || value > MAX_LENGTH_MM)
		return null;
	return value;
}

export function scaleFromLength(
	millimetres: number,
	pixels: number,
	width: number,
	height: number,
): CameraScale | null {
	if (
		!Number.isFinite(millimetres) ||
		millimetres <= 0 ||
		millimetres > MAX_LENGTH_MM
	) {
		return null;
	}
	if (!Number.isFinite(pixels) || pixels < MIN_LINE_PX) return null;
	if (!Number.isFinite(width) || width < 1 || width > 20000) return null;
	if (!Number.isFinite(height) || height < 1 || height > 20000) return null;
	const mmPerPx = millimetres / pixels;
	if (!Number.isFinite(mmPerPx) || mmPerPx <= 0 || mmPerPx > MAX_MM_PER_PX)
		return null;
	return { mmPerPx, width, height };
}

/** Millimetres per pixel of an image `imageWidth` wide that shows the same view. */
export function effectiveMmPerPx(
	scale: CameraScale,
	imageWidth: number,
): number | null {
	if (!Number.isFinite(imageWidth) || imageWidth < 1) return null;
	if (!Number.isFinite(scale.mmPerPx) || scale.mmPerPx <= 0) return null;
	if (!Number.isFinite(scale.width) || scale.width < 1) return null;
	const next = scale.mmPerPx * (scale.width / imageWidth);
	if (!Number.isFinite(next) || next <= 0) return null;
	return next;
}

export function lengthMm(
	pixels: number,
	imageWidth: number,
	scale: CameraScale,
): number | null {
	if (!Number.isFinite(pixels) || pixels < 0) return null;
	const mmPerPx = effectiveMmPerPx(scale, imageWidth);
	if (mmPerPx === null) return null;
	return pixels * mmPerPx;
}

export function formatPixels(pixels: number): string {
	return `${Math.round(pixels)} px`;
}

export function formatLengthMm(mm: number): string {
	if (mm >= 100) return `${mm.toFixed(0)} mm`;
	if (mm >= 10) return `${mm.toFixed(1)} mm`;
	return `${mm.toFixed(2)} mm`;
}

export function formatMmPerPx(mmPerPx: number): string {
	if (mmPerPx >= 10) return `${mmPerPx.toFixed(1)} mm/px`;
	if (mmPerPx >= 1) return `${mmPerPx.toFixed(2)} mm/px`;
	if (mmPerPx >= 0.01) return `${mmPerPx.toFixed(3)} mm/px`;
	const pxPerMm = 1 / mmPerPx;
	if (pxPerMm >= 10) return `${pxPerMm.toFixed(1)} px/mm`;
	return `${pxPerMm.toFixed(2)} px/mm`;
}

export function formatScale(scale: CameraScale, imageWidth: number): string {
	const mmPerPx = effectiveMmPerPx(
		scale,
		imageWidth > 0 ? imageWidth : scale.width,
	);
	if (mmPerPx === null) return "";
	return `Scale ${formatMmPerPx(mmPerPx)}`;
}

export function formatReadout(readout: MeasureReadout): string {
	if (readout.length) return `${readout.pixels} · ${readout.length}`;
	return `${readout.pixels}. ${readout.message ?? "No scale set for this camera."}`;
}

export function measureReadout(
	pixels: number,
	imageWidth: number,
	scale: CameraScale | null,
): MeasureReadout {
	const px = formatPixels(pixels);
	if (!scale) {
		return {
			pixels: px,
			length: null,
			message: "No scale set for this camera.",
		};
	}
	const mm = lengthMm(pixels, imageWidth, scale);
	if (mm === null) {
		return {
			pixels: px,
			length: null,
			message: "No scale set for this camera.",
		};
	}
	return { pixels: px, length: formatLengthMm(mm), message: null };
}

/** Where the image sits inside the pane. A zoom box already matches the image. */
export function measureFrameBox(
	zoom: FrameBox | null,
	frameWidth: number,
	frameHeight: number,
	imageWidth: number,
	imageHeight: number,
): FrameBox | null {
	if (imageWidth < 1 || imageHeight < 1) return null;
	if (zoom && zoom.width > 1 && zoom.height > 1) {
		return {
			left: zoom.left,
			top: zoom.top,
			width: zoom.width,
			height: zoom.height,
		};
	}
	if (frameWidth < 1 || frameHeight < 1) return null;
	const fit = Math.min(frameWidth / imageWidth, frameHeight / imageHeight);
	if (!Number.isFinite(fit) || fit <= 0) return null;
	const width = imageWidth * fit;
	const height = imageHeight * fit;
	return {
		left: (frameWidth - width) / 2,
		top: (frameHeight - height) / 2,
		width,
		height,
	};
}
