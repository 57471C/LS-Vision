import { getContext, setContext } from "svelte";
import { derived, type Readable, type Writable } from "svelte/store";
import type { CatalogMatch } from "$lib/catalog-match";
import {
	type DetectRoi,
	emptyDetectRois,
	soleDetectRoi,
} from "$lib/detect-rois";
import type { ConsoleLine } from "$lib/operator/console";

export type { DetectRoi };

export type Part = { ic: string; des: string; qty: number };

export type InspectView = {
	status: string;
	message?: string;
	kitCode?: string;
	jobCard?: string;
	des?: string;
	parts?: Part[];
	kitsFound?: number;
	expectedKitCount?: number;
	info?: string;
	image?: string;
};

export type SnapshotView = {
	kitCode: string;
	image: string;
	source: string;
	takenAt: string;
	detected?: boolean;
};

export type BarcodeRoi = {
	x: number;
	y: number;
	width: number;
	height: number;
};

export type Detection = {
	// Part IC from the YOLO class name. Kit codes come from the barcode lookup.
	ic: string;
	confidence: number;
	bbox: number[];
	// Set when a matching part rule used this box as its child.
	consumed?: boolean;
	// False when saved references for this class scored at or below the threshold.
	// Absent when that class has no reference, so the box still counts.
	referenceMatch?: boolean;
	// Catalogue IC drawn on the box. The BoM count still uses `ic`.
	catalogMatch?: CatalogMatch;
};

export type SlotDetections = {
	slot: number;
	detections: Detection[];
};

export type BomView = {
	kitCode: string;
	des?: string;
	parts: Part[];
};

export type DraftMark = {
	left: number;
	top: number;
	width: number;
	height: number;
};

export type ZoomFrame = {
	left: number;
	top: number;
	width: number;
	height: number;
};

export type DetectState = "idle" | "running" | "done";
export type BomSource = "json" | "mssql" | "api";

const DEFAULT_SCAN_SECONDS = 8;

export type OperatorState = {
	barcode: string;
	bomSource: BomSource;
	result: InspectView | null;
	bom: BomView | null;
	loading: boolean;
	cameraIndex: number;
	cameraIndexes: number[];
	camerasReady: boolean;
	cameraLookupFailed: boolean;
	feedOn: boolean;
	feedBusy: boolean;
	snapshot: SnapshotView | null;
	detections: Detection[];
	detectMessage: string;
	detectState: DetectState;
	testMode: boolean;
	admin: boolean;
	detectRoi: DetectRoi | null;
	detectRois: (DetectRoi | null)[];
	detectSlot: number;
	slotResults: SlotDetections[];
	barcodeRoi: BarcodeRoi | null;
	draftMark: DraftMark | null;
	drawing: boolean;
	barcodeDrawing: boolean;
	savingRoi: boolean;
	savingBarcodeRoi: boolean;
	roiMessage: string;
	paneWidth: number;
	paneHeight: number;
	frameWidth: number;
	frameHeight: number;
	streamLeft: number;
	liveMessage: string;
	livePainted: boolean;
	erpName: string;
	erpUrl: string;
	erpOpen: boolean;
	iframeFailed: boolean;
	erpChecking: boolean;
	erpLoadedUrl: string;
	modelName: string;
	barcodeDecoder: string;
	modelFiles: string[];
	modelMenuOpen: boolean;
	savingModel: boolean;
	modelNotice: string;
	partRuleWarning: boolean;
	partDescriptions: Record<string, string>;
	consoleOpen: boolean;
	consoleLines: ConsoleLine[];
};

export function initialOperator(): OperatorState {
	return {
		barcode: "",
		bomSource: "json",
		result: null,
		bom: null,
		loading: false,
		cameraIndex: 0,
		cameraIndexes: [],
		camerasReady: false,
		cameraLookupFailed: false,
		feedOn: true,
		feedBusy: false,
		snapshot: null,
		detections: [],
		detectMessage: "",
		detectState: "idle",
		testMode: false,
		admin: false,
		detectRoi: null,
		detectRois: emptyDetectRois(),
		detectSlot: 0,
		slotResults: [],
		barcodeRoi: null,
		draftMark: null,
		drawing: false,
		barcodeDrawing: false,
		savingRoi: false,
		savingBarcodeRoi: false,
		roiMessage: "",
		paneWidth: 0,
		paneHeight: 0,
		frameWidth: 0,
		frameHeight: 0,
		streamLeft: DEFAULT_SCAN_SECONDS,
		liveMessage: "",
		livePainted: false,
		erpName: "",
		erpUrl: "",
		erpOpen: false,
		iframeFailed: false,
		erpChecking: false,
		erpLoadedUrl: "",
		modelName: "best.onnx",
		barcodeDecoder: "",
		modelFiles: [],
		modelMenuOpen: false,
		savingModel: false,
		modelNotice: "",
		partRuleWarning: false,
		partDescriptions: {},
		consoleOpen: false,
		consoleLines: [],
	};
}

function zoomFrame(
	drawing: boolean,
	detectRois: readonly (DetectRoi | null)[],
	paneWidth: number,
	paneHeight: number,
	frameWidth: number,
	frameHeight: number,
): ZoomFrame | null {
	// While drawing, keep the whole still so the new box can start anywhere.
	// More than one saved slot stays on the full frame so every box is visible.
	const detectRoi = soleDetectRoi(detectRois);
	if (drawing || !detectRoi || !detectRoi.enabled) return null;
	if (paneWidth < 1 || paneHeight < 1 || frameWidth < 1 || frameHeight < 1)
		return null;
	const roiW = detectRoi.w * paneWidth;
	const roiH = detectRoi.h * paneHeight;
	if (roiW < 1 || roiH < 1) return null;
	const scale = Math.min(frameWidth / roiW, frameHeight / roiH);
	const roiViewW = roiW * scale;
	const roiViewH = roiH * scale;
	return {
		left: (frameWidth - roiViewW) / 2 - detectRoi.x * paneWidth * scale,
		top: (frameHeight - roiViewH) / 2 - detectRoi.y * paneHeight * scale,
		width: paneWidth * scale,
		height: paneHeight * scale,
	};
}

// Values from 0 to 1 are a portion of the frame. Anything larger is pixels.
export function pixelBarcodeRoi(
	roi: BarcodeRoi | null,
	imageWidth: number,
	imageHeight: number,
): BarcodeRoi | null {
	if (!roi || imageWidth < 1 || imageHeight < 1) return null;
	const { x, y, width, height } = roi;
	const portion =
		x >= 0 && y >= 0 && x <= 1 && y <= 1 && width <= 1 && height <= 1;
	const left = portion ? x * imageWidth : x;
	const top = portion ? y * imageHeight : y;
	const boxW = portion ? width * imageWidth : width;
	const boxH = portion ? height * imageHeight : height;
	if (boxW <= 0 || boxH <= 0) return null;
	const x0 = Math.max(0, Math.min(imageWidth - 1, Math.floor(left)));
	const y0 = Math.max(0, Math.min(imageHeight - 1, Math.floor(top)));
	const x1 = Math.max(x0 + 1, Math.min(imageWidth, Math.ceil(left + boxW)));
	const y1 = Math.max(y0 + 1, Math.min(imageHeight, Math.ceil(top + boxH)));
	const croppedW = x1 - x0;
	const croppedH = y1 - y0;
	if (croppedW <= 0 || croppedH <= 0) return null;
	return { x: x0, y: y0, width: croppedW, height: croppedH };
}

export type OperatorRefs = {
	paneImage: HTMLImageElement | null;
	paneFrame: HTMLElement | null;
	liveCanvas: HTMLCanvasElement | undefined;
	imagePicker: HTMLInputElement | null;
	erpCloseButton: HTMLButtonElement | null;
};

// Handlers the page and the three panels call. The session module fills them in.
export type OperatorSession = {
	refs: OperatorRefs;
	start: () => void;
	stop: () => void;
	runInspect: () => Promise<void>;
	saveCameraIndex: (next: number) => Promise<void>;
	decodeAndInspect: () => Promise<void>;
	toggleFeed: () => void;
	beginSetRoi: () => void;
	clearDetectRoi: () => void;
	setDetectSlot: (slot: number) => void;
	toggleDetectSlot: (slot: number) => void;
	beginSetBarcodeRoi: () => void;
	clearBarcodeRoi: () => void;
	openErp: () => Promise<void>;
	rememberPaneSize: (image?: HTMLImageElement | null) => void;
	resumeLiveFeed: () => void;
	onRoiPointerDown: (event: PointerEvent) => void;
	onRoiPointerMove: (event: PointerEvent) => void;
	onRoiPointerUp: (event: PointerEvent) => void;
	chooseAndDetect: () => Promise<void>;
	chooseAndSaveReferences: () => Promise<void>;
	onImagePicked: (event: Event) => void;
	onImageCancel: () => void;
	toggleModelMenu: () => Promise<void>;
	closeModelMenu: () => void;
	chooseModel: (filename: string) => Promise<void>;
	closeErp: () => void;
	onErpKey: (event: KeyboardEvent) => void;
	onIframeError: () => void;
	openErpInBrowser: () => Promise<void>;
	toggleConsole: () => void;
	clearConsole: () => void;
	dismissPartRuleWarning: () => void;
};

export type OperatorStores = {
	operator: Writable<OperatorState>;
	camerasMissing: Readable<boolean>;
	cameraFeedOn: Readable<boolean>;
	zoom: Readable<ZoomFrame | null>;
	barcodeOverlay: Readable<BarcodeRoi | null>;
} & OperatorSession;

const OPERATOR = Symbol("ls-vision-operator");

export function createOperatorViews(operator: Writable<OperatorState>) {
	// The live view reads these stores so the pane updates when the camera list arrives.
	const camerasMissing = derived(
		operator,
		(state) => state.camerasReady && state.cameraIndexes.length === 0,
	);
	const cameraFeedOn = derived(
		operator,
		(state) =>
			(state.camerasReady &&
				state.cameraIndexes.includes(Number(state.cameraIndex))) ||
			(state.cameraLookupFailed && !state.camerasReady),
	);
	// {@const zoom = zoomFrame()} is compiled with untrack, so a saved region never zoomed.
	const zoom = derived(operator, (state) =>
		zoomFrame(
			state.drawing,
			state.detectRois,
			state.paneWidth,
			state.paneHeight,
			state.frameWidth,
			state.frameHeight,
		),
	);
	const barcodeOverlay = derived(operator, (state) =>
		pixelBarcodeRoi(state.barcodeRoi, state.paneWidth, state.paneHeight),
	);
	return { camerasMissing, cameraFeedOn, zoom, barcodeOverlay };
}

export function provideOperator(stores: OperatorStores): void {
	setContext(OPERATOR, stores);
}

export function useOperator(): OperatorStores {
	const stores = getContext<OperatorStores | undefined>(OPERATOR);
	if (!stores) throw new Error("Operator store is missing");
	return stores;
}
