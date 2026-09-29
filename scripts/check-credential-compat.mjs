import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import {
  createPrivateKey,
  createPublicKey,
  randomBytes,
  randomUUID,
} from 'node:crypto';
import { once } from 'node:events';
import { mkdir, writeFile, unlink } from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

// All credentials belong to an isolated, disposable loopback network.
// Do not use this probe with production credentials: its CLI comparison uses argv.
const exec = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const binaries = path.join(root, 'src-tauri/binaries/easytier');
const work = path.join(root, 'target/credential-compat', randomUUID());
const network = `rela-credential-probe-${randomUUID()}`;
const processes = [];
const files = [];
const cleanEnv = Object.fromEntries(
  Object.entries(process.env).filter(([name]) => !/^ET_/i.test(name)),
);
await mkdir(work, { recursive: true });

async function freePort() {
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function cli(port, name, ...args) {
  try {
    const { stdout } = await exec(
      path.join(binaries, 'easytier-cli.exe'),
      [
        '--rpc-portal',
        `127.0.0.1:${port}`,
        '--instance-name',
        name,
        '--output',
        'json',
        ...args,
      ],
      { cwd: work, env: cleanEnv, windowsHide: true, timeout: 3000 },
    );
    return JSON.parse(stdout);
  } catch {
    throw new Error(
      'Probe RPC unavailable or returned an unexpected response.',
    );
  }
}

async function start(name, rpc, toml, extra = []) {
  const file = path.join(work, `${name}.toml`);
  const config = [
    `instance_name = "${name}"`,
    `instance_id = "${randomUUID()}"`,
    'dhcp = false',
    'stun_servers = []',
    'stun_servers_v6 = []',
    ...toml,
    '[flags]',
    'no_tun = true',
    'private_mode = true',
    'disable_p2p = true',
    'enable_ipv6 = false',
    'bind_device = false',
    'disable_upnp = true',
    'disable_udp_hole_punching = true',
    'disable_tcp_hole_punching = true',
  ].join('\n');
  await writeFile(file, config);
  files.push(file);
  const child = spawn(
    path.join(binaries, 'easytier-core.exe'),
    [
      '--disable-env-parsing',
      '--config-file',
      file,
      '--rpc-portal',
      `127.0.0.1:${rpc}`,
      '--rpc-portal-whitelist',
      '127.0.0.1/32',
      '--console-log-level',
      'warn',
      ...extra,
    ],
    {
      cwd: work,
      env: cleanEnv,
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  );
  child.probeName = name;
  // Core startup output can include private keys. Retain only a known error flag.
  child.authRejected = false;
  child.missingPrivateKey = false;
  for (const stream of [child.stdout, child.stderr]) {
    let tail = '';
    stream.on('data', (bytes) => {
      tail = (tail + bytes.toString()).slice(-8192);
      if (tail.includes('invalid proof and unknown credential'))
        child.authRejected = true;
      if (tail.includes('local private key is not set'))
        child.missingPrivateKey = true;
    });
  }
  processes.push(child);
  await once(child, 'spawn');
  await until(async () => Boolean(await cli(rpc, name, 'node')), 10_000);
  return child;
}

async function until(check, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      if (await check()) return;
    } catch {
      /* RPC may still be starting. */
    }
    await delay(200);
  }
  throw new Error('Isolated credential probe timed out.');
}

async function stop(child) {
  if (child.exitCode === null && child.signalCode === null) {
    const exited = once(child, 'exit');
    child.kill();
    await exited;
  }
}

function publicKey(secret) {
  const key = createPrivateKey({
    key: Buffer.concat([
      Buffer.from('302e020100300506032b656e04220420', 'hex'),
      Buffer.from(secret, 'base64'),
    ]),
    format: 'der',
    type: 'pkcs8',
  });
  return createPublicKey(key)
    .export({ format: 'der', type: 'spki' })
    .subarray(-32)
    .toString('base64');
}

function findCredential(value) {
  if (value && typeof value === 'object') {
    if (typeof value.credential_secret === 'string') return value;
    for (const child of Object.values(value)) {
      const found = findCredential(child);
      if (found) return found;
    }
  }
}

try {
  const { stdout: version } = await exec(
    path.join(binaries, 'easytier-core.exe'),
    ['--version'],
    { windowsHide: true, env: cleanEnv, timeout: 3000 },
  );
  const reports = [];
  for (const serverMode of ['toml-enabled-only', 'toml-explicit-keys']) {
    const listen = await freePort();
    const serverRpc = await freePort();
    const credentialFile = path.join(work, `${serverMode}-credentials.json`);
    files.push(credentialFile);
    const serverPrivateKey = randomBytes(32).toString('base64');
    const serverKeys =
      serverMode === 'toml-explicit-keys'
        ? [
            `local_private_key = "${serverPrivateKey}"`,
            `local_public_key = "${publicKey(serverPrivateKey)}"`,
          ]
        : [];
    const manager = await start(`manager-${serverMode}`, serverRpc, [
      `listeners = ["tcp://127.0.0.1:${listen}"]`,
      `credential_file = ${JSON.stringify(credentialFile)}`,
      '[network_identity]',
      `network_name = "${network}"`,
      `network_secret = "${randomBytes(24).toString('hex')}"`,
      '[secure_mode]',
      'enabled = true',
      ...serverKeys,
    ]);
    const credential = findCredential(
      await cli(
        serverRpc,
        manager.probeName,
        'credential',
        'generate',
        '--ttl',
        '120',
      ),
    );
    assert.ok(
      credential,
      'Credential generation must return a test credential.',
    );
    const key = publicKey(credential.credential_secret);
    const cases = [];
    const modes =
      serverMode === 'toml-enabled-only'
        ? ['cli-credential']
        : ['cli-credential', 'toml-only', 'toml-rela-args', 'toml-invalid'];
    for (const mode of modes) {
      const rpc = await freePort();
      const toml = [
        'listeners = []',
        '[network_identity]',
        `network_name = "${network}"`,
        '[[peer]]',
        `uri = "tcp://127.0.0.1:${listen}"`,
      ];
      const extra =
        mode === 'cli-credential'
          ? [
              '--secure-mode',
              '--credential',
              credential.credential_secret,
              '--no-listener',
            ]
          : mode === 'toml-rela-args' || mode === 'toml-invalid'
            ? ['--no-listener']
            : [];
      const secret =
        mode === 'toml-invalid'
          ? randomBytes(32).toString('base64')
          : credential.credential_secret;
      if (mode !== 'cli-credential')
        toml.push(
          '[secure_mode]',
          'enabled = true',
          `local_private_key = "${secret}"`,
          `local_public_key = "${mode === 'toml-invalid' ? publicKey(secret) : key}"`,
        );
      const name = `${serverMode}-${mode}`;
      const client = await start(name, rpc, toml, extra);
      let connected = false;
      let lastConnectors = [];
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) {
        const connectors = await cli(rpc, name, 'connector');
        lastConnectors = connectors;
        connected = connectors.some(
          (entry) => !!entry.url?.url && (entry.status ?? 0) === 0,
        );
        if (connected) break;
        await delay(250);
      }
      const node = await cli(rpc, name, 'node');
      assert.match(node.config, /no_tun\s*=\s*true/);
      cases.push({
        mode,
        connected,
        auth_rejected: client.authRejected,
        connector_statuses: lastConnectors.map((entry) => entry.status ?? 0),
      });
      await stop(client);
    }
    const serverNode = await cli(serverRpc, manager.probeName, 'node');
    reports.push({
      server_mode: serverMode,
      private_key_configured: /local_private_key\s*=/.test(serverNode.config),
      missing_private_key: manager.missingPrivateKey,
      cases,
    });
    await stop(manager);
  }
  const result = {
    core_version: version.trim(),
    loopback_only: true,
    no_tun: true,
    reports,
  };
  await writeFile(
    path.join(work, 'result.json'),
    `${JSON.stringify(result, null, 2)}\n`,
  );
  console.log(JSON.stringify(result, null, 2));
  for (const report of reports) {
    assert.equal(report.private_key_configured, true);
    assert.equal(report.missing_private_key, false);
    for (const entry of report.cases) {
      assert.equal(
        entry.connected,
        entry.mode !== 'toml-invalid',
        `Unexpected authentication result: ${entry.mode}`,
      );
      if (entry.connected) assert.equal(entry.auth_rejected, false);
    }
  }
} catch {
  console.error(
    'Credential probe failed; raw Core output and credentials were suppressed.',
  );
  process.exitCode = 1;
} finally {
  await Promise.all(processes.map(stop));
  await Promise.all(
    files.map(async (file) => {
      try {
        await unlink(file);
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
      }
    }),
  );
}
