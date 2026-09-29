import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { mkdir, unlink, writeFile } from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

// Explicit live-network check. Never run this from CI. The Rust helper decrypts
// the local DPAPI record and uses Rela's actual TOML serializer.
const exec = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const binaries = path.join(root, 'src-tauri/binaries/easytier');
const work = path.join(root, 'target/live-credential-probe', randomUUID());
const file = path.join(work, 'core.toml');
const credentialFile = path.resolve(
  process.argv[2] ?? path.join(root, 'config/credential.local.dat'),
);
const env = Object.fromEntries(
  Object.entries(process.env).filter(([key]) => !/^ET_|^RUST_LOG$/i.test(key)),
);
const result = {
  time_utc: new Date().toISOString(),
  authentication: 'TOML credential',
  no_tun: true,
  connected: false,
  stable_connection: false,
  auth_rejected: false,
  error_logged: false,
};
let child;
await mkdir(work, { recursive: true });

async function query(port, command) {
  const { stdout } = await exec(
    path.join(binaries, 'easytier-cli.exe'),
    [
      '--rpc-portal',
      `127.0.0.1:${port}`,
      '--instance-name',
      'rela',
      '--output',
      'json',
      command,
    ],
    {
      cwd: work,
      env,
      windowsHide: true,
      timeout: 3000,
      maxBuffer: 1024 * 1024,
    },
  );
  return JSON.parse(stdout);
}

try {
  await exec(
    path.join(root, 'target/debug/examples/credential-tool.exe'),
    ['probe-toml', credentialFile, file],
    { windowsHide: true, timeout: 5000 },
  );
  const { stdout: version } = await exec(
    path.join(binaries, 'easytier-core.exe'),
    ['--version'],
    { windowsHide: true, env, timeout: 3000 },
  );
  result.core_version = version.trim();
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  child = spawn(
    path.join(binaries, 'easytier-core.exe'),
    [
      '--disable-env-parsing',
      '--config-file',
      file,
      '--rpc-portal',
      `127.0.0.1:${port}`,
      '--rpc-portal-whitelist',
      '127.0.0.1/32',
      '--no-listener',
      '--console-log-level',
      'warn',
    ],
    { cwd: work, env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  for (const stream of [child.stdout, child.stderr]) {
    let tail = '';
    stream.on('data', (chunk) => {
      tail = (tail + chunk.toString())
        .slice(-8192)
        .replace(/\x1b\[[0-9;]*m/g, '');
      if (/invalid proof|unknown credential|private key is not set/i.test(tail))
        result.auth_rejected = true;
      if (/\bERROR\s/.test(tail)) result.error_logged = true;
    });
  }
  await once(child, 'spawn');
  const deadline = Date.now() + 35_000;
  let connectedSince;
  while (Date.now() < deadline && child.exitCode === null) {
    let connectors = [];
    try {
      connectors = await query(port, 'connector');
    } catch {
      /* RPC startup */
    }
    const connected =
      Array.isArray(connectors) &&
      connectors.some(
        (row) => !!row.url?.url && [0, 'CONNECTED'].includes(row.status ?? 0),
      );
    result.connected = connected;
    if (connected) {
      connectedSince ??= Date.now();
      if (Date.now() - connectedSince >= 5000) {
        result.stable_connection = true;
        break;
      }
    } else connectedSince = undefined;
    await delay(500);
  }
  const node = await query(port, 'node');
  result.virtual_ip = node.ipv4_addr || null;
  result.instance_id_matches =
    node.inst_id === '5f9e7c9b-747a-47e5-b62b-3c2b607c312e';
  result.no_tun_verified = /no_tun\s*=\s*true/.test(node.config ?? '');
  result.secure_mode_verified =
    /\[secure_mode\][\s\S]*?enabled\s*=\s*true/.test(node.config ?? '');
  result.no_network_secret = !/network_secret\s*=/.test(node.config ?? '');
  result.no_peer_public_key = !/peer_public_key\s*=/.test(node.config ?? '');
  const routes = await query(port, 'route');
  result.routes = routes.map((row) => ({
    hostname: row.hostname,
    ipv4: row.ipv4,
    path_len: row.path_len,
  }));
  assert.ok(result.connected && result.stable_connection && result.virtual_ip);
  assert.ok(
    result.instance_id_matches &&
      result.no_tun_verified &&
      result.secure_mode_verified,
  );
  assert.ok(result.no_network_secret && result.no_peer_public_key);
  assert.ok(!result.auth_rejected && !result.error_logged);
} catch {
  result.failed = true;
  process.exitCode = 1;
} finally {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exited = once(child, 'exit');
    child.kill();
    await exited;
  }
  await unlink(file).catch((error) => {
    if (error.code !== 'ENOENT') throw error;
  });
  result.probe_stopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  await writeFile(
    path.join(work, 'result.json'),
    `${JSON.stringify(result, null, 2)}\n`,
  );
  console.log(JSON.stringify(result, null, 2));
}
