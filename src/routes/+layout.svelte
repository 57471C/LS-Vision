<script lang="ts">
import { onMount } from "svelte";
import "../app.css";

type TauriWindow = Window & {
	__TAURI_INTERNALS__?: {
		invoke?: (command: string) => Promise<unknown>;
	};
};

// Release builds keep DevTools off unless the Tauri `devtools` feature is on.
// Ctrl+Shift+I is wired by Tauri. F12 is not, so open the same inspector here.
onMount(() => {
	function onKey(event: KeyboardEvent) {
		if (event.key !== "F12") return;
		const invoke = (window as TauriWindow).__TAURI_INTERNALS__?.invoke;
		if (typeof invoke !== "function") return;
		event.preventDefault();
		void invoke("plugin:webview|internal_toggle_devtools");
	}
	window.addEventListener("keydown", onKey);
	return () => window.removeEventListener("keydown", onKey);
});
</script>

<slot />
