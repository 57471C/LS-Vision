// Saved inspection rows and the pass/fail chart buckets.

export type ResultRange = "day" | "week" | "month";

export type ResultPart = {
	ic: string;
	des: string;
	qty: number;
	seen: number | null;
};

export type ResultExtra = { ic: string; count: number };

export type ResultDetail = {
	message?: string;
	info?: string;
	kitsFound?: number | null;
	expectedKitCount?: number | null;
	parts?: ResultPart[];
	extras?: ResultExtra[];
};

export type SavedResult = {
	id: number;
	passed: boolean;
	kitIc: string;
	jobCard: string;
	description: string;
	createdAt: string;
	hasImage: boolean;
	detail: ResultDetail;
};

export type ResultBuckets = {
	labels: string[];
	pass: number[];
	fail: number[];
};

function pad(value: number): string {
	return String(value).padStart(2, "0");
}

export function formatResultWhen(iso: string): string {
	const when = new Date(iso);
	if (Number.isNaN(when.getTime())) return iso;
	return `${pad(when.getDate())}/${pad(when.getMonth() + 1)}/${when.getFullYear()} ${pad(when.getHours())}:${pad(when.getMinutes())}:${pad(when.getSeconds())}`;
}

function sameDay(left: Date, right: Date): boolean {
	return (
		left.getFullYear() === right.getFullYear() &&
		left.getMonth() === right.getMonth() &&
		left.getDate() === right.getDate()
	);
}

function dayKey(when: Date): string {
	return `${when.getFullYear()}-${when.getMonth()}-${when.getDate()}`;
}

function countDays(rows: SavedResult[], days: Date[]): ResultBuckets {
	const labels = days.map(
		(day) => `${pad(day.getDate())}/${pad(day.getMonth() + 1)}`,
	);
	const pass = Array<number>(days.length).fill(0);
	const fail = Array<number>(days.length).fill(0);
	const index = new Map(days.map((day, slot) => [dayKey(day), slot]));
	for (const row of rows) {
		const when = new Date(row.createdAt);
		if (Number.isNaN(when.getTime())) continue;
		const slot = index.get(dayKey(when));
		if (slot === undefined) continue;
		if (row.passed) pass[slot] += 1;
		else fail[slot] += 1;
	}
	return { labels, pass, fail };
}

export function resultBuckets(
	rows: SavedResult[],
	range: ResultRange,
	now = new Date(),
): ResultBuckets {
	if (range === "day") {
		const labels = Array.from({ length: 24 }, (_, hour) => pad(hour));
		const pass = Array<number>(24).fill(0);
		const fail = Array<number>(24).fill(0);
		for (const row of rows) {
			const when = new Date(row.createdAt);
			if (Number.isNaN(when.getTime()) || !sameDay(when, now)) continue;
			if (row.passed) pass[when.getHours()] += 1;
			else fail[when.getHours()] += 1;
		}
		return { labels, pass, fail };
	}
	if (range === "week") {
		const days: Date[] = [];
		for (let offset = 6; offset >= 0; offset -= 1) {
			days.push(
				new Date(now.getFullYear(), now.getMonth(), now.getDate() - offset),
			);
		}
		return countDays(rows, days);
	}
	const last = new Date(now.getFullYear(), now.getMonth() + 1, 0).getDate();
	const end = Math.min(last, now.getDate());
	const days: Date[] = [];
	for (let day = 1; day <= end; day += 1) {
		days.push(new Date(now.getFullYear(), now.getMonth(), day));
	}
	return countDays(rows, days);
}

export function matchSummary(parts: ResultPart[]): string {
	const known = parts.filter((part) => part.seen !== null);
	if (!known.length) return "";
	let matched = 0;
	for (const part of known) {
		if (part.seen === part.qty) matched += 1;
	}
	const lines = known.length === 1 ? "line" : "lines";
	return `${matched} of ${known.length} BoM ${lines} match`;
}
