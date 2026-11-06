# Pixel-style character pet validation

The three replacement characters now use detailed pixel artwork generated from the existing British blue, Douhua and Huajuan cat style references. Eight poses per character remain connected to the existing task state engine.

- Engine regression suite: 49 passed, 0 failed (`cargo test --manifest-path src-tauri/Cargo.toml --lib pet_engine::`), full log in `pixel-engine-tests.log`.
- Settings UI suite: 111 checks passed, including all three generated previews and immediate selection persistence. Full report: `pixel-ui-tests.txt`.
- Asset checks: 24 APNGs, 48 decoded frames at 512×512; exact 4× integer pixel blocks, nonempty artwork and transparent 16px safety margins verified.
- Visual review: all eight poses, light/dark backgrounds, 48/72/96px sizes, alongside the original cat artwork.
- Original cat assets: SHA-256 hashes unchanged (`cat-reference-hashes.txt`, `cat-hash-check.txt`).

Active prompts, source sheets and runtime asset locations: `PROMPTS.md`. Previous 3D version is archived under `previous-3d/`.

Release build and persistent signature verification passed. Local app updated and restarted successfully. Settings were preserved and original cat hashes rechecked after restart. Previous application bundle: `../../dist/Agent Monitor.before-pixel-pets.app`.
