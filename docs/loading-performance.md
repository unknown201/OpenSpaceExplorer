# Remote loading

Remote loads were dominated by asset transfer and request ordering. The live server sent the KSC GLB
as 58,028,812 uncompressed bytes; the VAB GLB was another 47,848,500 bytes. Their material lists also
reference 151 MiB and 32 MiB of PNG textures. Launching every texture fetch at once queued later scene
models behind those images. Planet metadata additionally waited for a roughly 39 MiB terrain pack.

The loading path now:

- Serves precompressed gzip when present and uses fast streaming gzip for older exports. Decoded GLB,
  terrain, shader and WASM bytes are unchanged. Entry HTML revalidates; fingerprinted JS/WASM are immutable.
  Gzip and identity responses vary by Accept-Encoding, and range requests retain their original bytes.
- Builds material previews on the server, at most 256 pixels per side. PNG decoding is limited to two
  workers and 64 MiB per decode; encoded previews share a 32 MiB cache, invalidated by source size/mtime.
  Only model-material PNGs request previews: terrain masks, height maps, sky LUTs and physics data retain
  their full original inputs. Missing/unsupported previews fall back to the original file.
- Gives models the link before queued material images, limits preview requests to four, and upgrades one
  full-resolution texture at a time after its model's previews finish. Only currently drawn models start
  texture downloads. Switching scenes leaves at most the already active image requests to finish.
- Keeps 4–64 MiB GLB/terrain responses in a browser CacheStorage budget of 256 MiB. Each reuse sends
  If-Modified-Since and uses the stored body on 304. Web Locks serialize eviction across tabs. Blocked
  storage or failed writes fall back to regular downloads. Small material previews use normal HTTP caching.
- Starts planet metadata and texture discovery independently of terrain-pack transfer, and skips the
  binary pack request when its description is absent.
- Enables the Campaign button and pilot-name input only once their handlers are ready, so a quick
  interaction during startup cannot silently disappear while the rest of the client initializes.

Full-resolution source images are still fetched and displayed; previews make the scene usable first.
The runtime fallback works with existing exported assets, so a new game-asset export is not required.
`tools/precompress.py` prepares gzip sidecars for release bundles and asset uploads; CI, `deploy.py` and
`push_assets.py` invoke it automatically. Large scenes should be precompressed to avoid repeat CPU work.
Sidecars retain their source permissions, and release installation makes public web files readable by
the unprivileged server account.

## Measurements

Local Chromium/WebGPU samples on 2026-10-26, with 30 ms added network latency and one browser-wide
download cap. Fresh browser contexts provide cold loads; the second navigation in the same context
provides warm loads. These are short samples on this machine, not guarantees for every device or host.

| Scenario | Download budget | Result |
| --- | --- | --- |
| Original KSC + VAB model transfers only | 7.5 Mbps | 114.2 s cold; 114.0 s repeated |
| Same two models, streaming gzip fallback | 7.5 Mbps | 30.9 s cold |
| Full KSC-to-VAB flow, model compression/cache but full PNG flood | 7.5 Mbps | 323.6 s cold |
| KSC-to-VAB with previews, priority and caching | 7.5 Mbps | 64.1 s cold; 4.1 s warm |
| KSC-to-VAB with previews, priority and caching | 30 Mbps | 18.0 s cold; 1.25 s warm |

The last two flows wait for the current scene's material previews to reach the GPU. The older full-PNG
flow already took 323.6 seconds just to finish entering the VAB; model compression alone was insufficient.
Prepared model sidecars total about 23.2 MB, versus 105.9 MB of original GLBs. The 7.5 Mbps budget models
an equal quarter of a shared 30 Mbps uplink, without simulating four separate browsers or link contention.
Server previews were warm for the final samples. Full-resolution upgrades continue after scene readiness.

## Reproduce

Serve a release client and full `assets/web` from a private server. From `e2e`, in PowerShell:

```powershell
$env:OSE_BASE_URL = 'http://127.0.0.1:7788'
$env:OSE_DOWNLOAD_MBPS = '7.5'
$env:OSE_GAME = '1'
node scripts/profile-loading.mjs ../target/loading.json
```

Unset `OSE_GAME` to measure only the two model transfers. The script records scene response sizes and
conditional requests; it includes 304 headers even though Chromium does not emit loadingFinished for
those responses. WebGPU is required for the full scene flow.

Validation includes native HTTP tests for gzip round trips, precompressed pass-through, identity/range
requests, cache headers, preview dimensions, conditional responses and unchanged full-resolution images.
Browser tests cover persistent cache hits, asset updates, eviction, blocked storage, metadata concurrency
and preview-to-original upgrades. Headless Firefox's Canvas fallback skips WebGPU-only loading tests;
the regular gameplay tests cover its fallback path.
