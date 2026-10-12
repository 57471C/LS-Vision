<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  let barcode = "JOB-12345";
  let result: any = null;
  let error: string | null = null;
  let loading = false;
  let cameraIndex = 0;
  let decodeResult: any = null;

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
    error = null;
    result = null;
    try {
      const res = await fetch("http://127.0.0.1:8008/inspect", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ barcode })
      });
      if (!res.ok) throw new Error(await res.text());
      result = await res.json();
    } catch (err: any) {
      error = err.message || "Request failed";
    } finally {
      loading = false;
    }
  }

  async function decodeAndInspect() {
    try {
      const res = await fetch(`http://127.0.0.1:8008/camera/decode?index=${cameraIndex}`);
      const decoded = await res.json();
      decodeResult = decoded;

      if (decoded.status === "pass" && decoded.codes?.length) {
        barcode = decoded.codes[0].data;
        await runInspect();
      }
    } catch (err: any) {
      decodeResult = { status: "fail", message: err.message };
    }
  }
</script>

<div>
  <button on:click={startSidecar}>Start Vision Sidecar</button>

  <div style="margin-top: 1rem;">
    <label>
      Barcode:
      <input bind:value={barcode} />
    </label>
    <button on:click={runInspect} disabled={loading}>
      {loading ? "Inspecting..." : "Inspect"}
    </button>
  </div>

  {#if error}
    <p style="color: red;">{error}</p>
  {/if}

  {#if result}
    <div style="margin-top: 1rem; padding: 1rem; border: 1px solid #ccc;">
      <h3>Status: {result.status}</h3>
      <p>{result.message}</p>
      <pre>{JSON.stringify(result, null, 2)}</pre>
    </div>
  {/if}

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
    <button on:click={decodeAndInspect}>Decode QR / Barcode</button>

    {#if decodeResult}
      <pre>{JSON.stringify(decodeResult, null, 2)}</pre>
    {/if}
  </div>
</div>