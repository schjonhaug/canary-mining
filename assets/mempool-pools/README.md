# Mempool Mining Pool Registry

`pools-v2.json` is vendored from the mempool mining-pools registry and is used
for dashboard block pool attribution.

Source: https://github.com/mempool/mining-pools

The exact upstream commit, fetch time, and SHA-256 checksum are recorded in
`pools-v2.meta.json`.

The update script also mirrors available SVG logos from mempool.space production
assets into `ui/pool-logos/`. Logo fetch metadata and per-file checksums are
recorded in `pool-logos.meta.json`.

To update the vendored registry:

```bash
dev/update-mempool-pools.sh
```

Review the JSON, logo, and metadata diffs before committing. The app embeds the
registry and logo files at compile time, so runtime attribution stays
deterministic and does not depend on network access.
