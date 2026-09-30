import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { lstat, readdir, readFile, open } from 'node:fs/promises';
import path from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import {
  commonFiles,
  portableFiles,
  nsisPlugins,
  engineNames,
} from './release-layout.mjs';

const MAX_FILE = 512 * 1024 * 1024;
const MAX_TOTAL = 2 * 1024 * 1024 * 1024;
const MAX_ENTRIES = 128;
const MAX_LISTING = 1024 * 1024;
const TAIL = 8192;
const rules = [
  [
    'private_key_pem',
    /-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----/,
  ],
  [
    'minisign_private_key',
    /untrusted comment: (?:minisign (?:encrypted )?secret key|Rela private signing key)/i,
  ],
  [
    'github_token',
    /(?:gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{30,255})/,
  ],
  [
    'credential_assignment',
    /\b(?:credential_secret|network_secret|TAURI_SIGNING_PRIVATE_KEY|RELA_NETWORK_SECRET)\b["' \t]{0,8}[:=][ \t]{0,8}["']?[A-Za-z0-9+/=_-]{32,1024}/,
  ],
  [
    'private_json_key',
    /["'](?:private_key|secret_key)["'][ \t\r\n]{0,16}:[ \t\r\n]{0,16}["'][A-Za-z0-9+/=_-]{32,1024}["']/,
  ],
];
const dpapi = Buffer.from('01000000d08c9ddf0115d1118c7a00c04fc297eb', 'hex');
export class ScanFailure extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}
const fail = (code) => {
  throw new ScanFailure(code);
};
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

// Output is rule IDs only. No snippets, matching values or private member names
// enter stdout/reports, including errors returned by archive subprocesses.
export function createScanner(needles = []) {
  const encoded = needles.flatMap((needle) => {
    if (
      typeof needle !== 'string' ||
      needle.length < 8 ||
      Buffer.byteLength(needle) > 4096
    )
      fail('invalid_needle_file');
    return [Buffer.from(needle), Buffer.from(needle, 'utf16le')];
  });
  let tail = Buffer.alloc(0);
  const found = new Set();
  return {
    push(chunk) {
      const bytes = Buffer.concat([tail, chunk]);
      if (bytes.includes(dpapi)) found.add('dpapi_record');
      if (encoded.some((value) => bytes.includes(value)))
        found.add('configured_secret');
      for (const text of [
        bytes.toString('utf8'),
        bytes.toString('utf16le'),
        bytes.subarray(1).toString('utf16le'),
      ]) {
        for (const [id, rule] of rules) if (rule.test(text)) found.add(id);
      }
      tail = Buffer.from(bytes.subarray(Math.max(0, bytes.length - TAIL)));
    },
    finish() {
      return [...found].sort();
    },
  };
}

export async function plain(file, directory = false) {
  const actual = path.resolve(file);
  let current = actual;
  while (true) {
    const stat = await lstat(current).catch(() => fail('input_unavailable'));
    // Cargo normally hardlinks its executable from deps/. Reading a regular
    // hardlink is safe here: this scanner never writes through artifact paths.
    if (
      stat.isSymbolicLink() ||
      (current === actual && (directory ? !stat.isDirectory() : !stat.isFile()))
    )
      fail('unsafe_input_path');
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
}

async function consume(stream, needles, expectedSize = null, collect = false) {
  const scanner = createScanner(needles),
    hash = createHash('sha256');
  let size = 0;
  const chunks = [];
  for await (const chunk of stream) {
    size += chunk.length;
    if (size > MAX_FILE || (collect && size > 65536))
      fail('expanded_size_limit');
    hash.update(chunk);
    scanner.push(chunk);
    if (collect) chunks.push(chunk);
  }
  if (expectedSize !== null && size !== expectedSize)
    fail('archive_size_mismatch');
  return {
    size,
    sha256: hash.digest('hex'),
    rules: scanner.finish(),
    ...(collect ? { bytes: Buffer.concat(chunks) } : {}),
  };
}

async function run7z(executable, args, callback, timeoutMs = 120000) {
  const child = spawn(executable, args, {
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const exit = new Promise((resolve) => {
    child.once('error', () => resolve(false));
    child.once('close', (code) => resolve(code === 0));
  });
  // Drain without retaining stderr; archive diagnostics may contain sensitive
  // values as names. None of it is returned as a thrown child-process error.
  child.stderr.resume();
  const timer = setTimeout(() => child.kill(), timeoutMs);
  try {
    const result = await callback(child.stdout);
    if (!(await exit)) fail('archive_tool_failed');
    return result;
  } catch (error) {
    child.kill();
    await exit;
    throw error instanceof ScanFailure
      ? error
      : new ScanFailure('archive_tool_failed');
  } finally {
    clearTimeout(timer);
  }
}

export function parseListing(text, kind) {
  const records = text
    .trim()
    .split(/\r?\n\r?\n/)
    .filter(Boolean);
  if (records.length === 0 || records.length > MAX_ENTRIES)
    fail('archive_entry_limit');
  const seen = new Set();
  return records.map((block) => {
    const values = new Map();
    for (const line of block.split(/\r?\n/)) {
      const match = /^([^=\r\n]+) = (.*)$/.exec(line);
      if (!match || values.has(match[1])) fail('archive_listing_invalid');
      values.set(match[1], match[2]);
    }
    const original = values.get('Path');
    // 7-Zip on Windows renders ZIP separators as backslashes. Stored ZIP names
    // are checked independently below, before trusting this rendered listing.
    if (!original || /[\x00-\x1f\x7f]/.test(original))
      fail('archive_path_invalid');
    const name = original.replaceAll('\\', '/');
    const parts = name.replace(/\/$/, '').split('/');
    if (
      parts.some(
        (p) =>
          !p || p === '.' || p === '..' || /[:*?]/.test(p) || /[. ]$/.test(p),
      ) ||
      name.startsWith('/') ||
      !/^[ -~]+$/.test(name)
    )
      fail('archive_path_invalid');
    if (seen.has(name.toLowerCase())) fail('archive_duplicate_path');
    seen.add(name.toLowerCase());
    if (
      [...values.keys()].some((key) =>
        /link|alternate stream|reparse/i.test(key),
      ) ||
      values.get('Encrypted') === '+' ||
      /(?:^|\s)l[rwx-]{9}/.test(values.get('Attributes') ?? '')
    )
      fail('archive_special_entry');
    const directory =
      values.get('Folder') === '+' || /^D/.test(values.get('Attributes') ?? '');
    const rawSize = values.get('Size');
    const size =
      rawSize === '' && kind === 'installer' ? null : Number(rawSize);
    // NSIS solid archives sometimes omit the last member size; stdout is still
    // capped and compared with the exact compiled payload/pinned engine later.
    if (
      (size === null && kind !== 'installer') ||
      (size !== null &&
        (!/^[0-9]+$/.test(rawSize ?? '') ||
          !Number.isSafeInteger(size) ||
          size > MAX_FILE))
    )
      fail('archive_size_invalid');
    return { original, name, directory, size };
  });
}

// Read only bounded ZIP metadata. Decompression stays in 7-Zip's stdout pipe;
// no untrusted archive path is ever written into a directory. Reject ZIP64 and
// split archives: Rela's bounded, 128-member package format does not need them.
async function zipIndex(file) {
  const handle = await open(file, 'r');
  try {
    const size = (await handle.stat()).size;
    async function read(position, length) {
      if (
        position < 0 ||
        length < 0 ||
        length > MAX_LISTING ||
        position + length > size
      )
        fail('zip_metadata_invalid');
      const value = Buffer.alloc(length);
      let done = 0;
      while (done < length) {
        const result = await handle.read(
          value,
          done,
          length - done,
          position + done,
        );
        if (!result.bytesRead) fail('zip_metadata_invalid');
        done += result.bytesRead;
      }
      return value;
    }
    const tail = await read(Math.max(0, size - 65557), Math.min(size, 65557));
    let end = -1;
    for (let i = tail.length - 22; i >= 0; i--)
      if (
        tail.readUInt32LE(i) === 0x06054b50 &&
        i + 22 + tail.readUInt16LE(i + 20) === tail.length
      ) {
        end = i;
        break;
      }
    if (
      end < 0 ||
      tail.readUInt16LE(end + 4) !== 0 ||
      tail.readUInt16LE(end + 6) !== 0 ||
      tail.readUInt16LE(end + 8) !== tail.readUInt16LE(end + 10)
    )
      fail('zip_metadata_invalid');
    const count = tail.readUInt16LE(end + 10),
      length = tail.readUInt32LE(end + 12),
      offset = tail.readUInt32LE(end + 16);
    if (
      !count ||
      count > MAX_ENTRIES ||
      length > MAX_LISTING ||
      offset + length !== size - tail.length + end
    )
      fail('zip_metadata_invalid');
    const central = await read(offset, length),
      entries = [];
    let cursor = 0;
    for (let i = 0; i < count; i++) {
      if (
        cursor + 46 > central.length ||
        central.readUInt32LE(cursor) !== 0x02014b50
      )
        fail('zip_metadata_invalid');
      const flags = central.readUInt16LE(cursor + 8),
        method = central.readUInt16LE(cursor + 10),
        compressed = central.readUInt32LE(cursor + 20),
        expanded = central.readUInt32LE(cursor + 24);
      const names = central.readUInt16LE(cursor + 28),
        extra = central.readUInt16LE(cursor + 30),
        comment = central.readUInt16LE(cursor + 32),
        disk = central.readUInt16LE(cursor + 34),
        attributes = central.readUInt32LE(cursor + 38),
        local = central.readUInt32LE(cursor + 42);
      if (
        cursor + 46 + names + extra + comment > central.length ||
        (flags & 1) !== 0 ||
        disk !== 0 ||
        ![0, 8].includes(method) ||
        compressed > MAX_FILE ||
        expanded > MAX_FILE ||
        ((attributes >>> 16) & 0xf000) === 0xa000
      )
        fail('zip_metadata_invalid');
      const bytes = central.subarray(cursor + 46, cursor + 46 + names);
      let name;
      try {
        name = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
      } catch {
        fail('archive_path_invalid');
      }
      if (name.includes('\\') || name.includes('\0'))
        fail('archive_path_invalid');
      const header = await read(local, 30);
      if (
        header.readUInt32LE(0) !== 0x04034b50 ||
        header.readUInt16LE(6) !== flags ||
        header.readUInt16LE(8) !== method ||
        header.readUInt16LE(26) !== names
      )
        fail('zip_metadata_invalid');
      const localName = await read(local + 30, names);
      const dataEnd = local + 30 + names + header.readUInt16LE(28) + compressed;
      if (!bytes.equals(localName) || dataEnd > offset)
        fail('zip_metadata_invalid');
      entries.push({
        name,
        size: expanded,
        directory: name.endsWith('/'),
        start: local,
        end: dataEnd,
      });
      cursor += 46 + names + extra + comment;
    }
    if (cursor !== central.length) fail('zip_metadata_invalid');
    const regions = [...entries].sort((a, b) => a.start - b.start);
    if (regions.some((entry, i) => i > 0 && entry.start < regions[i - 1].end))
      fail('zip_metadata_invalid');
    return entries;
  } finally {
    await handle.close();
  }
}

function layout(entries, kind, version, profile) {
  if (
    !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version) ||
    !['debug', 'release'].includes(profile)
  )
    fail('invalid_scan_configuration');
  const stem = `Rela_${version}_x64-portable${profile === 'debug' ? '-debug' : ''}`;
  const expected =
    kind === 'installer'
      ? [
          ...commonFiles.map((f) => (f === 'Rela.exe' ? 'rela.exe' : f)),
          ...nsisPlugins.map((f) => `$PLUGINSDIR/${f}`),
        ]
      : [...portableFiles, 'checksums.json'].map((f) => `${stem}/${f}`);
  const allowed = new Set(expected),
    names = new Set(entries.filter((e) => !e.directory).map((e) => e.name));
  for (const entry of entries) {
    if (entry.directory) {
      if (
        !expected.some((name) =>
          name.startsWith(`${entry.name.replace(/\/$/, '')}/`),
        )
      )
        fail('unexpected_archive_directory');
    } else if (!allowed.has(entry.name)) fail('unexpected_archive_file');
  }
  if (expected.some((name) => !names.has(name))) fail('missing_archive_file');
  return stem;
}

function inspectMetadata(members, kind, stem, version, profile, pin) {
  const at = (name) =>
    members.get(kind === 'installer' ? name : `${stem}/${name}`);
  const json = (name) => {
    try {
      return JSON.parse(at(name).bytes.toString('utf8'));
    } catch {
      fail('artifact_metadata_invalid');
    }
  };
  const engine = json('easytier/manifest.json');
  if (!isDeepStrictEqual(engine, pin)) fail('engine_manifest_mismatch');
  for (const name of engineNames)
    if (at(`easytier/${name}`).sha256 !== pin.files[name])
      fail('engine_hash_mismatch');
  if (kind === 'installer') return;
  const index = json('checksums.json');
  const fields = ['version', 'profile', 'files'];
  if (!isDeepStrictEqual(Object.keys(index).sort(), fields.sort()))
    fail('checksum_index_mismatch');
  const expected = [...portableFiles].sort();
  if (
    index.version !== version ||
    index.profile !== profile ||
    JSON.stringify(Object.keys(index.files ?? {}).sort()) !==
      JSON.stringify(expected)
  )
    fail('checksum_index_mismatch');
  for (const name of expected)
    if (at(name).sha256 !== index.files[name]) fail('checksum_mismatch');
}

export async function scanArtifact({
  artifact,
  kind,
  version,
  profile = 'release',
  pin,
  needles = [],
  sevenZip = '7z',
  expectedBinary,
}) {
  const report = {
    schema_version: 1,
    kind,
    version,
    profile,
    coverage: [
      'raw_bytes',
      'utf8_utf16_rules',
      'configured_needles',
      ...(['installer', 'portable'].includes(kind)
        ? [
            'decompressed_payload',
            'fixed_file_inventory',
            'checksums_and_engine_pin',
          ]
        : []),
    ],
    complete: false,
    passed: false,
    artifacts: [],
    findings: [],
  };
  try {
    if (!['installer', 'portable', 'binary', 'log', 'frontend'].includes(kind))
      fail('invalid_scan_kind');
    await plain(artifact, kind === 'frontend');
    if (kind === 'frontend') {
      const files = [];
      async function walk(directory, relative = '', depth = 0) {
        if (depth > 16) fail('archive_entry_limit');
        for (const entry of await readdir(directory, { withFileTypes: true })) {
          const next = path.join(directory, entry.name),
            name = relative + entry.name;
          if (entry.isSymbolicLink()) fail('unsafe_input_path');
          const nameScan = createScanner(needles);
          nameScan.push(Buffer.from(name));
          if (nameScan.finish().length) fail('sensitive_member_name');
          if (entry.isDirectory()) await walk(next, `${name}/`, depth + 1);
          else {
            if (!/\.(?:js|css|html|svg|png|ico)$/.test(name))
              fail('unexpected_frontend_file');
            files.push([next, name]);
          }
          if (files.length > MAX_ENTRIES) fail('archive_entry_limit');
        }
      }
      await walk(artifact);
      if (!files.some(([, name]) => name === 'index.html'))
        fail('missing_frontend_entry');
      for (const [file, name] of files) {
        await plain(file);
        const result = await consume(createReadStream(file), needles);
        report.artifacts.push({
          member: name,
          size: result.size,
          sha256: result.sha256,
        });
        for (const rule of result.rules)
          report.findings.push({ member: name, rule });
      }
    } else {
      const raw = await consume(createReadStream(artifact), needles);
      report.artifacts.push({
        member: 'container',
        size: raw.size,
        sha256: raw.sha256,
      });
      for (const rule of raw.rules)
        report.findings.push({ member: 'container', rule });
      if (!['binary', 'log'].includes(kind)) {
        const zip = kind === 'installer' ? null : await zipIndex(artifact);
        const listing = await run7z(
          sevenZip,
          ['l', '-slt', '-ba', '-sccUTF-8', '--', path.resolve(artifact)],
          async (stream) => {
            let size = 0;
            const chunks = [];
            for await (const chunk of stream) {
              size += chunk.length;
              if (size > MAX_LISTING) fail('archive_entry_limit');
              chunks.push(chunk);
            }
            return Buffer.concat(chunks).toString('utf8');
          },
        );
        const entries = parseListing(listing, kind);
        if (
          zip &&
          (zip.length !== entries.length ||
            zip.some(
              (item, i) =>
                item.name !== entries[i].name ||
                item.size !== entries[i].size ||
                item.directory !== entries[i].directory,
            ))
        )
          fail('archive_listing_mismatch');
        const stem = layout(entries, kind, version, profile);
        const members = new Map();
        let expanded = 0;
        for (const entry of entries.filter((e) => !e.directory)) {
          const result = await run7z(
            sevenZip,
            [
              'e',
              '-so',
              '-spd',
              '-y',
              '--',
              path.resolve(artifact),
              entry.original,
            ],
            (stream) =>
              consume(
                stream,
                needles,
                entry.size,
                entry.name.endsWith('checksums.json') ||
                  entry.name.endsWith('easytier/manifest.json'),
              ),
          );
          expanded += result.size;
          if (expanded > MAX_TOTAL) fail('expanded_size_limit');
          members.set(entry.name, result);
          report.artifacts.push({
            member: entry.name,
            size: result.size,
            sha256: result.sha256,
          });
          for (const rule of result.rules)
            report.findings.push({ member: entry.name, rule });
        }
        inspectMetadata(members, kind, stem, version, profile, pin);
        if (expectedBinary) {
          await plain(expectedBinary);
          const expected = await consume(createReadStream(expectedBinary), []);
          const actual = members.get(
            kind === 'installer' ? 'rela.exe' : `${stem}/Rela.exe`,
          );
          if (actual.sha256 !== expected.sha256)
            fail('application_payload_mismatch');
        }
      }
    }
    report.complete = true;
    report.passed = report.findings.length === 0;
  } catch (error) {
    report.findings.push({
      rule: error instanceof ScanFailure ? error.code : 'scan_failed',
    });
  }
  return report;
}

export async function readNeedles(file) {
  if (!file) return [];
  await plain(file);
  const stat = await lstat(file);
  if (stat.size > 65536) fail('invalid_needle_file');
  let values;
  try {
    values = JSON.parse(await readFile(file, 'utf8'));
  } catch {
    fail('invalid_needle_file');
  }
  if (!Array.isArray(values) || values.length > 64) fail('invalid_needle_file');
  createScanner(values);
  return values;
}
export const sha256 = digest;
