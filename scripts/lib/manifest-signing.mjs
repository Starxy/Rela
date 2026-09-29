import { createPrivateKey, sign, verify } from 'node:crypto';

export function signingMessage(purpose, keyId, payload) {
  if (!['resources', 'software'].includes(purpose))
    throw new Error('清单用途无效。');
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(keyId) || payload.length > 512 * 1024)
    throw new Error('签名输入无效。');
  return Buffer.concat([
    Buffer.from(`Rela signed manifest v1\n${purpose}\n${keyId}\n`),
    payload,
  ]);
}

export function signManifest(payload, purpose, keyId, privateKey) {
  const key = createPrivateKey(privateKey);
  if (key.asymmetricKeyType !== 'ed25519')
    throw new Error('必须使用 Ed25519 密钥。');
  const message = signingMessage(purpose, keyId, payload);
  return {
    format: 'rela.signed.v1',
    key_id: keyId,
    payload: payload.toString('base64'),
    signature: sign(null, message, key).toString('base64'),
  };
}

export function verifyManifest(envelope, purpose, publicKey) {
  if (envelope.format !== 'rela.signed.v1') throw new Error('签名格式无效。');
  const payload = Buffer.from(envelope.payload, 'base64');
  if (
    !verify(
      null,
      signingMessage(purpose, envelope.key_id, payload),
      publicKey,
      Buffer.from(envelope.signature, 'base64'),
    )
  )
    throw new Error('清单签名无效。');
  return payload;
}
