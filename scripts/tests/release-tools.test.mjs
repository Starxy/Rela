import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm, realpath } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { deflateRawSync } from 'node:zlib';
import {
  createScanner,
  parseListing,
  scanArtifact,
  sha256,
  readNeedles,
} from '../lib/release-scan.mjs';
import {
  commonFiles,
  portableFiles,
  engineNames,
  assertResourceMap,
  resourceMap,
} from '../lib/release-layout.mjs';
import { selectSourcePaths, buildEnvironment } from '../lib/clean-inputs.mjs';

const canary = 'RELA_SYNTHETIC_TEST_ONLY_DO_NOT_USE_abcdefghijklmnopqrstuvwxyz';
function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) {
    value ^= byte;
    for (let i = 0; i < 8; i++)
      value = value & 1 ? (value >>> 1) ^ 0xedb88320 : value >>> 1;
  }
  return (value ^ 0xffffffff) >>> 0;
}
function zip(entries) {
  const bodies = [],
    headers = [];
  let offset = 0;
  for (const [name, value] of entries) {
    const raw = Buffer.from(name),
      bytes = Buffer.from(value),
      compressed = deflateRawSync(bytes),
      crc = crc32(bytes);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x800, 6);
    local.writeUInt16LE(8, 8);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(compressed.length, 18);
    local.writeUInt32LE(bytes.length, 22);
    local.writeUInt16LE(raw.length, 26);
    bodies.push(local, raw, compressed);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x800, 8);
    central.writeUInt16LE(8, 10);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(compressed.length, 20);
    central.writeUInt32LE(bytes.length, 24);
    central.writeUInt16LE(raw.length, 28);
    central.writeUInt32LE(offset, 42);
    headers.push(central, raw);
    offset += local.length + raw.length + compressed.length;
  }
  const directory = Buffer.concat(headers),
    end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...bodies, directory, end]);
}
async function fixture() {
  const directory = await mkdtemp(
    path.join(os.tmpdir(), 'rela-release-scan-test-'),
  );
  const stem = 'Rela_0.2.0_x64-portable-debug';
  const files = Object.fromEntries(
    portableFiles.map((name) => [
      name,
      Buffer.from(`public synthetic ${name}`),
    ]),
  );
  const pin = {
    version: 'fixture-core',
    engine_revision: 1,
    files: Object.fromEntries(
      engineNames.map((name) => [name, sha256(files[`easytier/${name}`])]),
    ),
  };
  files['easytier/manifest.json'] = Buffer.from(JSON.stringify(pin));
  async function write(extra = []) {
    const index = {
      version: '0.2.0',
      profile: 'debug',
      files: Object.fromEntries(
        Object.entries(files).map(([name, bytes]) => [name, sha256(bytes)]),
      ),
    };
    const entries = [
      ...Object.entries(files).map(([name, bytes]) => [
        `${stem}/${name}`,
        bytes,
      ]),
      [`${stem}/checksums.json`, JSON.stringify(index)],
      ...extra,
    ];
    const artifact = path.join(directory, 'package.zip');
    await writeFile(artifact, zip(entries));
    return artifact;
  }
  return {
    directory,
    files,
    stem,
    pin,
    write,
    async writeInstaller(extra = []) {
      const entries = Object.entries(files)
        .filter(([name]) => commonFiles.includes(name))
        .map(([name, bytes]) => [
          name === 'Rela.exe' ? 'rela.exe' : name,
          bytes,
        ]);
      const artifact = path.join(directory, 'installer-payload.zip');
      // ZIP fixtures exercise the payload rules through the same 7-Zip reader.
      await writeFile(artifact, zip([...entries, ...extra]));
      return artifact;
    },
    scan: (artifact, options = {}) =>
      scanArtifact({
        artifact,
        kind: 'portable',
        version: '0.2.0',
        profile: 'debug',
        pin,
        ...options,
      }),
    async cleanup() {
      const resolved = await realpath(directory);
      assert.equal(
        path.dirname(resolved).toLowerCase(),
        (await realpath(os.tmpdir())).toLowerCase(),
      );
      assert.ok(path.basename(resolved).startsWith('rela-release-scan-test-'));
      await rm(resolved, { recursive: true });
    },
  };
}

test('streaming scanner catches UTF8/UTF16/DPAPI and split secrets without echoing values', () => {
  const inputs = [
    Buffer.from(`credential_secret = "${canary}"`),
    Buffer.from(`network_secret="${canary}"`, 'utf16le'),
    Buffer.from('01000000d08c9ddf0115d1118c7a00c04fc297eb', 'hex'),
    Buffer.from(`-----BEGIN OPENSSH PRIVATE KEY-----\n${canary}`),
  ];
  for (const bytes of inputs) {
    const scanner = createScanner([canary]);
    for (let i = 0; i < bytes.length; i += 7)
      scanner.push(bytes.subarray(i, i + 7));
    const found = scanner.finish();
    assert.ok(found.length);
    assert.ok(!JSON.stringify(found).includes(canary));
  }
  const publicKey = createScanner();
  publicKey.push(
    Buffer.from('"public_key":"abcdefghijklmnopqrstuvwxyz0123456789="'),
  );
  assert.deepEqual(publicKey.finish(), []);
});

test('listing rejects duplicates, traversal, links, encryption, oversize and forged records', () => {
  const row = (name, other = '') =>
    `Path = ${name}\nSize = 12\nFolder = -\nEncrypted = -${other}\n`;
  for (const listing of [
    row('../private.dat'),
    row('C:/private.dat'),
    row('ok') + '\n' + row('OK'),
    row('ok', '\nSymbolic Link = target'),
    row('ok').replace('Encrypted = -', 'Encrypted = +'),
    row('ok').replace('Size = 12', 'Size = 9000000000'),
    row('ok', '\nPath = replacement'),
  ])
    assert.throws(() => parseListing(listing, 'portable'));
  const missing = parseListing(
    'Path = file\nSize = \nMethod = LZMA\n',
    'installer',
  );
  assert.equal(missing[0].size, null);
});

test('archive scanner decompresses allowlisted files and validates exact checksums and pinned engine', async () => {
  const f = await fixture();
  try {
    let artifact = await f.write();
    assert.equal((await f.scan(artifact)).passed, true);
    f.files['README.txt'] = Buffer.from(
      `hidden compressed credential_secret="${canary}"`,
    );
    artifact = await f.write();
    const report = await f.scan(artifact);
    assert.equal(report.complete, true);
    assert.equal(report.passed, false);
    assert.ok(report.findings.some((v) => v.rule === 'credential_assignment'));
    assert.ok(!JSON.stringify(report).includes(canary));
    f.files['README.txt'] = Buffer.from('public text');
    f.files['easytier/wintun.dll'] = Buffer.from('different engine');
    artifact = await f.write();
    const changed = await f.scan(artifact);
    assert.equal(changed.complete, false);
    assert.equal(changed.findings.at(-1).rule, 'engine_hash_mismatch');
  } finally {
    await f.cleanup();
  }
});

test('installer scans generated files without requiring fixed NSIS internals', async () => {
  const f = await fixture();
  try {
    for (const generated of [
      [],
      [
        ['uninstall.exe', 'generated uninstaller'],
        ['$PLUGINSDIR/new-helper.dll', 'generated plugin'],
      ],
    ]) {
      const report = await f.scan(await f.writeInstaller(generated), {
        kind: 'installer',
      });
      assert.equal(report.complete, true);
      assert.equal(report.passed, true);
      for (const [name] of generated)
        assert.ok(report.artifacts.some((item) => item.member === name));
    }
  } finally {
    await f.cleanup();
  }
});

test('installer still checks required product resources and generated file contents', async () => {
  const f = await fixture();
  try {
    const report = await f.scan(
      await f.writeInstaller([
        ['uninstall.exe', `credential_secret="${canary}"`],
      ]),
      { kind: 'installer' },
    );
    assert.equal(report.complete, true);
    assert.equal(report.passed, false);
    assert.ok(
      report.findings.some(
        (item) =>
          item.member === 'uninstall.exe' &&
          item.rule === 'credential_assignment',
      ),
    );
    assert.ok(!JSON.stringify(report).includes(canary));
    delete f.files['easytier/easytier-core.exe'];
    const missing = await f.scan(await f.writeInstaller(), {
      kind: 'installer',
    });
    assert.equal(missing.complete, false);
    assert.equal(missing.findings.at(-1).rule, 'missing_archive_file');
  } finally {
    await f.cleanup();
  }
});

test('unknown private archive members fail without exposing member names; traversal never extracts', async () => {
  const f = await fixture();
  try {
    const secretName = `${f.stem}/credentials/${canary}.dat`;
    let report = await f.scan(await f.write([[secretName, 'secret']]));
    assert.equal(report.passed, false);
    assert.equal(report.findings.at(-1).rule, 'unexpected_archive_file');
    assert.ok(!JSON.stringify(report).includes(canary));
    report = await f.scan(await f.write([['../outside-file', 'secret']]));
    assert.equal(report.passed, false);
    assert.ok(
      ['archive_path_invalid', 'archive_listing_mismatch'].includes(
        report.findings.at(-1).rule,
      ),
    );
    report = await f.scan(
      await f.write([[`${f.stem}\\credential.dat`, 'secret']]),
    );
    assert.equal(report.findings.at(-1).rule, 'archive_path_invalid');
  } finally {
    await f.cleanup();
  }
});

test('wrong binary, corrupt ZIP header and missing extractor never receive a passing report', async () => {
  const f = await fixture();
  try {
    const artifact = await f.write(),
      binary = path.join(f.directory, 'wrong.exe');
    await writeFile(binary, 'wrong executable');
    assert.equal(
      (await f.scan(artifact, { expectedBinary: binary })).findings.at(-1).rule,
      'application_payload_mismatch',
    );
    assert.equal(
      (
        await f.scan(artifact, {
          sevenZip: path.join(f.directory, 'missing-extractor'),
        })
      ).passed,
      false,
    );
    const bytes = await readFile(artifact);
    bytes[30] ^= 1;
    await writeFile(artifact, bytes);
    assert.equal(
      (await f.scan(artifact)).findings.at(-1).rule,
      'zip_metadata_invalid',
    );
  } finally {
    await f.cleanup();
  }
});

test('needle file parser is bounded and configured values are never placed in findings', async () => {
  const f = await fixture();
  try {
    const file = path.join(f.directory, 'needles.json');
    await writeFile(file, JSON.stringify([canary]));
    assert.deepEqual(await readNeedles(file), [canary]);
    await writeFile(file, JSON.stringify(['short']));
    await assert.rejects(() => readNeedles(file));
  } finally {
    await f.cleanup();
  }
});

test('clean input selection excludes local configuration, runtime data, private keys and build output', () => {
  const selected = selectSourcePaths([
    'src/App.tsx',
    'src-tauri/src/lib.rs',
    'Cargo.toml',
    'config/distribution.json',
    'config/network.local.json',
    'config/credential.local.dat',
    'target/release/rela.exe',
    'data/config/credentials/a.dat',
    '.env',
    '.codex/notes.md',
    'src-tauri/binaries/easytier/config.toml',
    'third-party-licenses/private-key.pem',
  ]);
  assert.deepEqual(selected, [
    'Cargo.toml',
    'config/distribution.json',
    'src-tauri/src/lib.rs',
    'src/App.tsx',
  ]);
  assert.throws(() => selectSourcePaths(['../escape.rs']));
  const env = buildEnvironment(
    {
      Path: 'tool-path',
      APPDATA: 'public-location',
      TAURI_SIGNING_PRIVATE_KEY: canary,
      GH_TOKEN: canary,
      VITE_PRIVATE: canary,
      RUSTFLAGS: canary,
    },
    'clean-target',
    'synthetic',
  );
  assert.equal(env.Path, 'tool-path');
  assert.equal(env.CARGO_TARGET_DIR, 'clean-target');
  assert.ok(!JSON.stringify(env).includes(canary));
});

test('resource allowlist matches packaging and rejects directory globs', async () => {
  const tauri = JSON.parse(
    await readFile(
      new URL('../../src-tauri/tauri.conf.json', import.meta.url),
      'utf8',
    ),
  );
  assertResourceMap(tauri);
  assert.throws(() =>
    assertResourceMap({
      bundle: { resources: { ...resourceMap, '../data/': 'data/' } },
    }),
  );
  assert.deepEqual(
    Object.values(resourceMap).sort(),
    portableFiles
      .filter(
        (name) => !['Rela.exe', 'README.txt', 'portable.txt'].includes(name),
      )
      .sort(),
  );
});
