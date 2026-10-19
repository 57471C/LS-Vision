// In-app operator console. The session appends lines. The panel renders them.

export const CONSOLE_LIMIT = 100;

export type ConsoleLine = {
	id: number;
	time: string;
	text: string;
};

export function consoleStamp(now: Date): string {
	const pad = (value: number, size = 2) => String(value).padStart(size, "0");
	return `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}.${pad(now.getMilliseconds(), 3)}`;
}

// Drop the oldest lines once the buffer is past the limit.
export function appendConsoleLine(
	lines: readonly ConsoleLine[],
	line: ConsoleLine,
): ConsoleLine[] {
	const next = lines.concat(line);
	if (next.length <= CONSOLE_LIMIT) return next;
	return next.slice(next.length - CONSOLE_LIMIT);
}

// Rust camera failures are prefixed with "Camera". Encode errors are not.
export function isCameraOpenError(message: string): boolean {
	return /^camera\b/i.test(message.trim());
}
