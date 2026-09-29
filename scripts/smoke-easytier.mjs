import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const binaries = path.join(root, 'src-tauri', 'binaries', 'easytier');
const work = path.join(root, 'target', 'core-smoke', randomUUID());
await mkdir(work, { recursive: true });
const processes = [];
const coreVersion = JSON.parse(
  await readFile(path.join(root, 'config', 'easytier-version.json'), 'utf8'),
).version;

async function freePort() {
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function start(
  name,
  address,
  rpc,
  listen,
  peers,
  secret = 'rela-smoke-credential',
) {
  const config = [
    `instance_name = "${name}"`,
    `instance_id = "${randomUUID()}"`,
    `ipv4 = "${address}"`,
    'dhcp = false',
    `listeners = [${listen ? `"tcp://127.0.0.1:${listen}"` : ''}]`,
    'stun_servers = []',
    'stun_servers_v6 = []',
    '[network_identity]',
    'network_name = "rela-isolated-smoke"',
    `network_secret = "${secret}"`,
    ...peers.flatMap((peer) => ['[[peer]]', `uri = "${peer}"`]),
    '[flags]',
    'private_mode = true',
    'disable_p2p = true',
    'no_tun = true',
    'enable_ipv6 = false',
    'bind_device = false',
    'disable_upnp = true',
    'disable_udp_hole_punching = true',
    'disable_tcp_hole_punching = true',
  ].join('\n');
  const file = path.join(work, `${name}.toml`);
  await writeFile(file, config);
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
    ],
    { cwd: binaries, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  child.smokeName = name;
  child.smokeLog = '';
  for (const stream of [child.stdout, child.stderr])
    stream.on('data', (chunk) => {
      child.smokeLog = (child.smokeLog + chunk.toString()).slice(-3000);
    });
  processes.push(child);
  await once(child, 'spawn');
  return child;
}

async function query(port, name, command) {
  const { stdout } = await exec(
    path.join(binaries, 'easytier-cli.exe'),
    [
      '--rpc-portal',
      `127.0.0.1:${port}`,
      '--instance-name',
      name,
      '--output',
      'json',
      command,
    ],
    { cwd: binaries, windowsHide: true, timeout: 2000, maxBuffer: 1024 * 1024 },
  );
  return JSON.parse(stdout);
}

async function until(check, timeout = 15_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      if (await check()) return;
    } catch {
      /* RPC is starting. */
    }
    await delay(250);
  }
  throw new Error('隔离 Core 测试超时。');
}

async function stop(child) {
  if (child.exitCode === null && child.signalCode === null) {
    const exited = once(child, 'exit');
    child.kill();
    await exited;
  }
}

try {
  const [listen, serverRpc, clientRpc, mismatchRpc] = await Promise.all([
    freePort(),
    freePort(),
    freePort(),
    freePort(),
  ]);
  await start('rela-smoke-server', '10.254.254.1', serverRpc, listen, []);
  const client = await start(
    'rela-smoke-client',
    '10.254.254.2',
    clientRpc,
    null,
    [`tcp://127.0.0.1:${listen}`],
  );
  await until(async () =>
    (await query(clientRpc, 'rela-smoke-client', 'connector')).some(
      (item) => !!item.url?.url && (item.status ?? 0) === 0,
    ),
  );
  const node = await query(clientRpc, 'rela-smoke-client', 'node');
  assert.equal(node.version, coreVersion, '实际 RPC 版本必须匹配随包版本');
  assert.match(node.ipv4_addr, /^10\.254\.254\.2(?:\/\d+)?$/);
  assert.match(node.config, /private_mode\s*=\s*true/);
  assert.match(node.config, /disable_p2p\s*=\s*true/);
  assert.match(node.config, /no_tun\s*=\s*true/);
  const routes = await query(clientRpc, 'rela-smoke-client', 'route');
  assert.ok(Array.isArray(routes));
  assert.ok(routes.every((route) => typeof route.path_len === 'number'));
  await stop(client);
  let rpcStopped = false;
  try {
    await query(clientRpc, 'rela-smoke-client', 'node');
  } catch {
    rpcStopped = true;
  }
  assert.equal(rpcStopped, true);
  await start(
    'rela-smoke-mismatch',
    '10.254.254.3',
    mismatchRpc,
    null,
    [`tcp://127.0.0.1:${listen}`],
    'different-smoke-credential',
  );
  await until(async () => {
    await query(mismatchRpc, 'rela-smoke-mismatch', 'node');
    return true;
  });
  await delay(1500);
  assert.ok(
    (await query(mismatchRpc, 'rela-smoke-mismatch', 'connector')).every(
      (item) => (item.status ?? 0) !== 0,
    ),
  );
  console.log(
    'Core 隔离验证通过：本机连接、默认开关、RPC JSON、停止和错误密钥拒绝。未创建虚拟网卡或连接实验室网络。',
  );
} catch (error) {
  for (const child of processes)
    console.error(
      `${child.smokeName}: exit=${child.exitCode}\n${child.smokeLog}`,
    );
  throw error;
} finally {
  await Promise.all(processes.map(stop));
}
