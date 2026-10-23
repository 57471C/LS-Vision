// Optional class rewrite after detection. Rules come from config.
// An empty list leaves every class as the model named it.
// Rules run once, top to bottom, against those original class names.
// The first match renames that parent box. A result IC is not matched again.

export const PART_RULE_LIMIT = 40;
export const PART_LABEL_LIMIT = 80;

export type PartCondition = "contains" | "no-child";

export type PartRule = {
	parent: string;
	child: string;
	condition: PartCondition;
	result: string;
};

export type PartBox = {
	ic: string;
	confidence: number;
	bbox: number[];
};

export type PartComposition = {
	detections: PartBox[];
	log: string[];
};

type Rect = {
	x1: number;
	y1: number;
	x2: number;
	y2: number;
};

export function conditionLabel(condition: PartCondition): string {
	return condition === "no-child" ? "no child" : "contains";
}

function cleanLabel(value: unknown): string {
	if (typeof value !== "string") return "";
	const text = value.trim();
	if (!text || text.length > PART_LABEL_LIMIT) return "";
	return text;
}

function readCondition(value: unknown): PartCondition | null {
	if (value === "contains") return "contains";
	if (value === "no-child" || value === "no child") return "no-child";
	return null;
}

// Drop rows that are missing a class, a condition, or a result IC.
export function parsePartRules(value: unknown): PartRule[] {
	if (!Array.isArray(value)) return [];
	const rules: PartRule[] = [];
	for (const item of value) {
		if (!item || typeof item !== "object") continue;
		const row = item as Record<string, unknown>;
		const parent = cleanLabel(row.parent);
		const child = cleanLabel(row.child);
		const result = cleanLabel(row.result);
		const condition = readCondition(row.condition);
		if (!parent || !child || !result || !condition) continue;
		rules.push({ parent, child, condition, result });
		if (rules.length >= PART_RULE_LIMIT) break;
	}
	return rules;
}

function rectOf(bbox: number[]): Rect | null {
	if (bbox.length < 4) return null;
	const [rawX1, rawY1, rawX2, rawY2] = bbox;
	if (
		![rawX1, rawY1, rawX2, rawY2].every(
			(value) => typeof value === "number" && Number.isFinite(value),
		)
	) {
		return null;
	}
	const x1 = Math.min(rawX1, rawX2);
	const x2 = Math.max(rawX1, rawX2);
	const y1 = Math.min(rawY1, rawY2);
	const y2 = Math.max(rawY1, rawY2);
	if (x2 <= x1 || y2 <= y1) return null;
	return { x1, y1, x2, y2 };
}

// The child counts as inside when its center lies in the parent, edges included.
function centerInside(child: Rect, parent: Rect): boolean {
	const x = (child.x1 + child.x2) / 2;
	const y = (child.y1 + child.y2) / 2;
	return x >= parent.x1 && x <= parent.x2 && y >= parent.y1 && y <= parent.y2;
}

function formatBox(box: Rect): string {
	const n = (value: number) =>
		Number.isInteger(value) ? String(value) : value.toFixed(1);
	return `${n(box.x1)},${n(box.y1)},${n(box.x2)},${n(box.y2)}`;
}

export function applyPartRules(
	detections: readonly PartBox[],
	rules: readonly PartRule[],
): PartComposition {
	if (rules.length === 0) {
		return { detections: detections.slice(), log: [] };
	}
	const boxes = detections.map((item) => rectOf(item.bbox));
	const next = detections.slice();
	const log: string[] = [];
	for (let index = 0; index < detections.length; index += 1) {
		const item = detections[index];
		const parentBox = boxes[index];
		if (!parentBox) continue;
		for (let ruleIndex = 0; ruleIndex < rules.length; ruleIndex += 1) {
			const rule = rules[ruleIndex];
			if (item.ic !== rule.parent) continue;
			const inside = detections.some((other, otherIndex) => {
				if (otherIndex === index || other.ic !== rule.child) return false;
				const childBox = boxes[otherIndex];
				if (!childBox) return false;
				return centerInside(childBox, parentBox);
			});
			const matched = rule.condition === "contains" ? inside : !inside;
			if (!matched) continue;
			next[index] = { ...item, ic: rule.result };
			log.push(
				`Part rule ${ruleIndex + 1} matched: ${rule.parent} ${conditionLabel(rule.condition)} ${rule.child} -> ${rule.result} at ${formatBox(parentBox)}`,
			);
			break;
		}
	}
	if (log.length === 0) log.push("Part rules: none matched");
	return { detections: next, log };
}
