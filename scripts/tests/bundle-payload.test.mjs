import test from 'node:test';
import assert from 'node:assert/strict';
import { nsisPayload } from '../lib/bundle-payload.mjs';

function executable() {
  const bytes = Buffer.alloc(512);
  bytes.write('MZ');
  bytes.writeUInt32LE(64, 60);
  bytes.writeUInt32LE(0x4550, 64);
  bytes.writeUInt16LE(0x8664, 68);
  bytes.writeUInt16LE(0x20b, 88);
  bytes.write('__TAURI_BUNDLE_TYPE_VAR_UNK', 400);
  return bytes;
}

test('ZIP payload matches the NSIS marker change without mutating the compiler output', () => {
  const source = executable();
  const untouched = Buffer.from(source);
  const expected = Buffer.from(source);
  expected.write('__TAURI_BUNDLE_TYPE_VAR_NSS', 400);
  assert.deepEqual(nsisPayload(source), expected);
  assert.deepEqual(source, untouched);
});

test('payload preparation rejects signed, ambiguous, already bundled and non-x64 inputs', () => {
  const signed = executable();
  signed.writeUInt32LE(100, 64 + 24 + 112 + 32);
  const repeated = executable();
  repeated.write('__TAURI_BUNDLE_TYPE_VAR_UNK', 450);
  const x86 = executable();
  x86.writeUInt16LE(0x14c, 68);
  for (const bytes of [
    signed,
    repeated,
    x86,
    nsisPayload(executable()),
    Buffer.alloc(1),
  ]) {
    assert.throws(() => nsisPayload(bytes), /invalid_unbundled_application/);
  }
});
