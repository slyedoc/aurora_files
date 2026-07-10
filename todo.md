Noted, low value — fine to leave:
- asset.rs has two "TODO: use async and get rid of this adapter" block_on shims — upstream bevy's meshlet asset has the identical pattern; not worth diverging.
- The SOLARI_* debug env levers (PTLAS_VALIDATE, VALIDATE, XFORM_DEBUG, CAMERA_DEBUG, TESS_SCALE, OMM_GATE-era ones are gone) are few enough now; a one-place inventory in a module doc would be nice-to-have only.
- The exam harness isn't duplicated after all — only furnace carries ExamAppExt, so nothing to extract.

Already parked from earlier:
- The intermittent vkAcquireNextImageKHR semaphore VUID (~2 in 20 launches, in memory as a follow-up; worth re-checking with a long soak since the teardown fixes landed).
- The June backlog proper: per-frame material/light buffer rebuilds → Tier-2 persistence, atmosphere bake caching, arena eviction (which is also what turns MeshOmm's teardown-only Drop into a deferred retire).
