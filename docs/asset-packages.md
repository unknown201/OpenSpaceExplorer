# Assets by usage

The server builds a versioned package catalog from the installed exported assets. It serves
`/assets/packages/index.json` with revalidation and compressed binary bundles at revision-specific
URLs. No asset re-export, archive upload or hand-maintained list is required to enable packages on
an existing installation. The catalog builds lazily off the server's request threads and refreshes
after 30 seconds; browser startup also refreshes its catalog.

| Usage | Contents | Speculative next usage |
| --- | --- | --- |
| Main menu | Common UI, menu backdrop, then KSC and VAB | No planet surfaces |
| Mission control | Common UI; mission data already comes over the game protocol | VAB, then KSC |
| KSC / launchpad | Campus metadata, model and small material previews | VAB |
| VAB | Interior metadata, model and previews | Kerbin's essential flight assets |
| Planet / moon | That body's metadata, surface maps and atmosphere inputs, with shared flight data | No other bodies |

High-resolution planet detail layers, scatter masks and local terrain binaries are in the body's
on-demand chunks. Small planet/terrain/scatter descriptions lead its essential chunks so renderers
can discover their inputs immediately. Existing rendering requests determine when detail chunks load. Craft parts
and full-resolution model-material upgrades keep their individual on-demand URLs; opening the VAB
does not download every available rocket part. Shared material previews have a separate dependency
group so a common texture never requires another scene's large model.

On the current local export, common UI occupies one bundle, KSC three and VAB two, compared with
190 KSC and 44 VAB image references plus their models/metadata. Mun has one essential bundle and
four further chunks for on-demand assets, plus the common flight dependency. These counts can change
with an asset export. The local catalog builds in about 150 ms.

Bundles target 8 MiB of decoded inputs, using a preview-size estimate for material images. A large
model/file occupies its own chunk (with small leading metadata) rather than splitting its bytes
across requests. Files above 64 MiB stay on the ordinary loader. Essential planet assets and detail
assets occupy separate chunks, so a metadata request cannot inadvertently pull in every surface
detail layer. Server gzip uses level 6; the outer fast-compression fallback does not encode it again.
Completed encoded bundles share a 96 MiB server cache, with a 32 MiB capture limit per building
bundle. An aborted or failed stream is never admitted. The existing 32 MiB preview cache is separate.
The server prepares common UI, shared material, menu, KSC and VAB bundles in a background task at startup,
without using a player's download link. Other bundles are prepared when first requested. A player
arriving during preparation can still stream a demanded bundle; server startup never waits for it.

## Browser scheduling and caching

The menu's first paint includes a static vector planet/starfield and a fallback wordmark in the HTML,
adding about 1.5 KiB gzip in the local build. No image request, JavaScript execution or exported game
asset is required to display it. The package bootstrap is deferred so a slow script download cannot
block that paint. Pilot-name entry and campaign selection become usable independently of scenery.

The scenery loader is a seven-piece ship assembly illustration with a progress track. It remains
indeterminate until the menu description arrives, then follows actual required models: geometry
uploaded and initial material previews ready. Reused part IDs count once. The final window/flame and
100% appear only when every required model is textured, immediately before the existing backdrop fade.
It does not advance on a timer, wait for full-resolution upgrades or block the campaign controls.
Reduced-motion settings disable the assembly movement and scanning animation.

A separate muted corner meter shows other asset packs, with friendly destination names and a ready
count. Completed entries and bytes received within the current entry determine chunk progress; a small
remainder is reserved until that entry has been cached. Chunk fractions are averaged within each
destination, then across destinations: this is loading-work progress, not an ETA or a percentage of
all compressed network bytes. The core/menu groups are omitted from this secondary meter. It appears
only after 250 ms of pending work, stays still on a stalled link, and hides when no work remains.
Cached instant loads avoid a flash. Demanded detail chunks also report after preloading completes.
Updates are throttled to roughly ten per second, with serialized completion notifications delivered
before the worker task settles. Worker upgrades reconnect the active usage and its progress listener.

The sky, menu planet, clouds and their material previews form a `menu` package, prioritized ahead of
speculative KSC/VAB transfers. The selected craft reuses its ordinary part assets on demand; unselected
menu vessels are not downloaded. A separate presentation-ready check waits for every drawn backdrop
layer and active selected-craft part to have its initial material previews, then fades the static
artwork out over 650 ms. Full-resolution upgrades continue normally. Reduced-motion users get an
immediate handoff. With no WebGPU or no exported menu scene, the static artwork remains; it is hidden
when entering a campaign so it never covers the game.

An early bootstrap registers a service worker on HTTPS/localhost. Once WebGPU is ready, the client
reports its current screen or flight body's ID. The worker queues the active usage ahead of likely
next usages. Changing destination drops queued speculative chunks from the old plan. One speculative
transfer runs at a time, with capacity for a demanded chunk alongside it. Demanded requests promote
queued work and share already-running transfers instead of downloading the same bundle again.
An active speculative transfer finishes; switching destinations does not discard/restart its bytes.

The worker reads framed entries incrementally and persists them under their original asset URLs.
Metadata becomes available before the rest of a model's bundle finishes. Geometry parsing, image
decoding and GPU uploads remain in their existing renderers when those assets are actually used.
Package unpacking does not run on the rendering thread. Frame headers and entry sizes are checked
before allocations, and malformed/missing packages fall back to the ordinary file request.

Package members use a 256 MiB persistent-cache budget. If storage fails, a separate 128 MiB memory
budget allows reuse for the worker's lifetime. The ordinary loader retains its own bounded cache
for unbundled large assets; packaged members are not copied into that cache again. Each member is
validated against its chunk revision, including updates within a Last-Modified second. HTTP range
requests and URLs outside the catalog retain the normal network path. Insecure origins, blocked
service workers, missing catalogs and installations without exported assets keep the original loader
and menu prefetching behavior.

Revision tokens include member paths, source sizes/mtimes and the preview format version. They are
cache invalidation tokens, not cryptographic integrity hashes. Normal export tools must update a
changed source's size or modification time. Old cached immutable bundles remain usable under their
old revision URL; new catalogs point at the new export.

## Wire format and validation

After HTTP decompression, a bundle begins with `OSEPACK1`. Entries have a little-endian u32 JSON
header length, UTF-8 metadata (`url`, `size`, MIME `type`, `preview`, `modified`), and exactly `size`
body bytes. A zero header length terminates the bundle. GLB, terrain and source-image bytes are
unchanged; only the existing explicitly requested material previews use smaller WebP images.

Native HTTP tests cover gzip round trips, exact model bytes, preview dimensions, encoded-cache reuse,
gzip refusal, revision changes, dependency isolation, missing revisions and oversized-file fallback.
Browser tests run the real worker in Chromium and Firefox with deterministic HTTP fixtures. They
cover usage order, on-demand detail, streaming metadata, shared transfers, reprioritization, reloads,
storage failure, invalid lengths, stale revisions, eviction and range bypass. Existing renderer
regressions run with service workers blocked to continue testing the ordinary fallback explicitly.

For transport comparisons, use the release server and the loading profiler in `e2e`:

```powershell
$env:OSE_BASE_URL = 'http://127.0.0.1:PORT'
$env:OSE_GAME = '1'
$env:OSE_DOWNLOAD_MBPS = '30'
$env:OSE_MENU_WAIT_MS = 'ready'
node scripts/profile-loading.mjs ../target/packages.json
$env:OSE_PACKAGES = '0'
node scripts/profile-loading.mjs ../target/individual.json
```

The game profiler uses a local reverse proxy to apply one compressed-HTTP-byte budget and 30 ms
added request latency across both page and worker traffic. Page-target CDP throttling alone does not
limit worker requests. WebSocket campaign traffic is forwarded normally. The menu craft selection is
fixed between comparisons; the report separates setup from scene transitions and includes package
resource timings plus all proxied HTTP body bytes. Browser member caches survive between samples;
workers are stopped between samples so unfinished speculative traffic does not contaminate the next
run. Earlier direct-CDP measurements remain samples of the individual-loader path, not worker traffic.

Release-server samples at 30 Mbps / 30 ms with a fixed menu craft, a cold browser and prepared server
bundles measured 15.46 s in the menu and 17.33 s through textured KSC then VAB. The individual-file
fallback measured 18.24 s and 19.57 s respectively. Repeated navigation measured 1.62 s total with
packages and 1.85 s individually. These are short samples, not multi-player/device guarantees.
Total proxied HTTP bodies were about 60 MB in either cold flow, including the menu backdrop and its
craft: grouping primarily removes request overhead and controls usage order rather than removing the
underlying asset data. Both requested scenes were usable about a second after preloading finished.

Building gzip bundles on an unprepared server added substantial one-time work in the initial sample
(29 s menu preparation). Common bundles are now prepared during server startup and retained for reuse;
an immediate arrival during preparation can still pay that cost. Other bodies are prepared on their
first request. No speed claim here relies on the earlier unthrottled worker measurements.
