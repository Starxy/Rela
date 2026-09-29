// Synthetic fixture key only. Never used by production trust configuration.
import { createPrivateKey, createPublicKey } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { signManifest } from './lib/manifest-signing.mjs';

const directory = new URL(
  '../crates/rela-manifests/tests/fixtures/',
  import.meta.url,
);
const privateKey = createPrivateKey({
  key: Buffer.concat([
    Buffer.from('302e020100300506032b657004220420', 'hex'),
    Buffer.alloc(32, 42),
  ]),
  type: 'pkcs8',
  format: 'der',
});
const pem = privateKey.export({ type: 'pkcs8', format: 'pem' });
const publicKey = createPublicKey(privateKey)
  .export({ type: 'spki', format: 'der' })
  .subarray(-32)
  .toString('base64');
const input = await readFile(
  new URL('../config/resources.example.json', import.meta.url),
);
const envelope = signManifest(input, 'resources', 'synthetic-test-key', pem);
await mkdir(directory, { recursive: true });
await writeFile(
  new URL('keys.json', directory),
  JSON.stringify(
    [{ id: 'synthetic-test-key', purpose: 'resources', public_key: publicKey }],
    null,
    2,
  ) + '\n',
);
await writeFile(
  new URL('resources.signed.json', directory),
  JSON.stringify(envelope, null, 2) + '\n',
);
console.log('已生成跨语言验签测试样本（非生产密钥）。');
