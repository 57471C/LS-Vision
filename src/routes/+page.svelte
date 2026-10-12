<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  async function startSidecar() {
    try {
      const result = await invoke("start_vision_sidecar");
      console.log(result);
    } catch (err) {
      console.error(err);
    }
  }
</script>

<button on:click={startSidecar}>Start Vision Sidecar</button>