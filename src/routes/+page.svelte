<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { invoke } from "@tauri-apps/api/core";

type Part = { ic: string; des: string; qty: number };
type InspectView = {
	status: string;
	message?: string;
	kitCode?: string;
	parts?: Part[];
	kitsFound?: number;
	expectedKitCount?: number;
};

let barcode = "JOB-12345";
let result: InspectView | null = null;
let loading = false;
let cameraIndex = 0;

function errorMessage(err: unknown, fallback: string): string {
	return err instanceof Error && err.message ? err.message : fallback;
}

async function startSidecar() {
	try {
		const res = await invoke("start_vision_sidecar");
		console.log(res);
	} catch (err) {
		console.error(err);
	}
}

async function runInspect() {
	loading = true;
	result = null;
	try {
		const res = await fetch("http://127.0.0.1:8008/inspect", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ barcode }),
		});
		if (!res.ok) throw new Error(await res.text());
		result = await res.json();
	} catch (err: unknown) {
		result = { status: "fail", message: errorMessage(err, "Request failed") };
	} finally {
		loading = false;
	}
}

async function decodeAndInspect() {
	try {
		const res = await fetch(
			`http://127.0.0.1:8008/camera/decode?index=${cameraIndex}`,
		);
		const decoded = await res.json();

		if (decoded.status === "pass" && decoded.codes?.length) {
			barcode = decoded.codes[0].data;
			await runInspect();
		} else {
			result = {
				status: "fail",
				message: decoded.message || "No code detected",
			};
		}
	} catch (err: unknown) {
		result = { status: "fail", message: errorMessage(err, "Decode failed") };
	}
}
</script>

<div>
  <button type="button" on:click={startSidecar}>Start Vision Sidecar</button>

  <div style="margin-top: 1rem;">
    <label>
      Barcode:
      <input bind:value={barcode} />
    </label>
    <button type="button" on:click={runInspect} disabled={loading}>
      {loading ? "Inspecting..." : "Inspect"}
    </button>
  </div>

  <div style="margin-top: 1rem;">
    <label>
      Camera:
      <select bind:value={cameraIndex}>
        <option value={0}>0 (default)</option>
        <option value={1}>1</option>
      </select>
    </label>
    <br />
    <img
      src="http://127.0.0.1:8008/camera/stream?index={cameraIndex}"
      alt="camera"
      style="max-width: 480px; border: 1px solid #ccc; margin-top: 0.5rem;"
    />
    <br />
    <button type="button" on:click={decodeAndInspect} disabled={loading}>Decode QR / Barcode</button>
  </div>

  {#if result}
    <section class="panel" class:pass={result.status === "pass"} class:fail={result.status !== "pass"}>
      <p class="status">{result.status}</p>
      {#if result.kitCode}
        <p class="kit">Kit {result.kitCode}</p>
      {/if}
      {#if result.message}
        <p class="message">{result.message}</p>
      {/if}
      {#if result.parts?.length}
        <table>
          <thead>
            <tr>
              <th>IC</th>
              <th>Description</th>
              <th>Qty</th>
            </tr>
          </thead>
          <tbody>
            {#each result.parts as part, i (i)}
              <tr>
                <td>{part.ic}</td>
                <td>{part.des}</td>
                <td>{part.qty}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
      {#if result.kitsFound != null || result.expectedKitCount != null}
        <p class="info">
          Info only: {result.kitsFound ?? "—"} kit(s) found, {result.expectedKitCount ?? "—"} expected
        </p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .panel {
    margin-top: 1rem;
    padding: 1rem;
    border: 3px solid #ccc;
    max-width: 36rem;
  }

  .panel.pass {
    border-color: #1b7f3a;
    background: #e8f6ec;
  }

  .panel.fail {
    border-color: #b42318;
    background: #fdecea;
  }

  .status {
    margin: 0;
    font-size: 3rem;
    font-weight: 800;
    line-height: 1;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .panel.pass .status {
    color: #1b7f3a;
  }

  .panel.fail .status {
    color: #b42318;
  }

  .kit {
    margin: 0.75rem 0 0;
    font-size: 1.25rem;
    font-weight: 700;
  }

  .message {
    margin: 0.5rem 0 0;
  }

  table {
    width: 100%;
    margin-top: 0.75rem;
    border-collapse: collapse;
  }

  th,
  td {
    padding: 0.35rem 0.5rem;
    text-align: left;
    border-bottom: 1px solid rgba(0, 0, 0, 0.12);
  }

  th {
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  td:last-child,
  th:last-child {
    text-align: right;
  }

  .info {
    margin: 0.75rem 0 0;
    color: #444;
    font-size: 0.95rem;
  }
</style>
