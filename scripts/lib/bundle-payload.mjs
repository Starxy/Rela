// Match the one bundle-type change performed by Tauri CLI 2.11.5 for NSIS.
// The installed updater verifies its NSIS helper against the companion ZIP,
// so both delivery forms must contain this exact executable. Rela selects
// Portable mode using portable.txt, independently of Tauri's bundle marker.
// Upstream: tauri-cli-v2.11.5/crates/tauri-bundler/src/bundle.rs
const originalMarker = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK');
const nsisMarker = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_NSS');

export function nsisPayload(source) {
  const invalid = () => {
    throw new Error('invalid_unbundled_application');
  };
  if (source.length < 64 || source.toString('ascii', 0, 2) !== 'MZ') invalid();
  const pe = source.readUInt32LE(60);
  if (
    pe + 264 > source.length ||
    source.readUInt32LE(pe) !== 0x4550 ||
    source.readUInt16LE(pe + 4) !== 0x8664 ||
    source.readUInt16LE(pe + 24) !== 0x20b
  )
    invalid();
  // This pipeline signs packages with minisign, not the EXE with Authenticode.
  // Never silently invalidate an Authenticode certificate if one is added.
  const certificate = pe + 24 + 112 + 4 * 8;
  if (source.readBigUInt64LE(certificate) !== 0n) invalid();
  const offset = source.indexOf(originalMarker);
  if (offset < 0 || source.indexOf(originalMarker, offset + 1) !== -1)
    invalid();
  const payload = Buffer.from(source);
  nsisMarker.copy(payload, offset);
  return payload;
}
