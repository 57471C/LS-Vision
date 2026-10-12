<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  let barcode = "JOB-12345";
  let result: any = null;
  let error: string | null = null;
  let loading = false;

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
</div>